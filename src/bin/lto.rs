use betterjeb::*;
use betterjeb::util::countdown;

use krpc_mars::{batch_call, batch_call_common, StreamHandle};
use std::error::Error;

const TURN_START_ALT: f64 = 250.;
const TURN_END_ALT: f64 = 45_000.;
const TARGET_ALTITUDE: f64 = 74_000.;
const SRB_FUEL: &str = "SolidFuel";

fn main() -> Result<()> {
    env_logger::init();

    log::info!("Connecting to kRPC.");
    log::debug!("Starting RPC connection");
    let client = krpc_mars::RPCClient::connect("Launch into Orbit (Rust)", "192.168.0.40:50000")?;
    log::debug!("Starting Stream connection");
    let stream_client = krpc_mars::StreamClient::connect(&client, "192.168.0.40:50001")?;

    log::info!("Preparing to launch into low-kerbin orbit.");
    log::info!("Planned flight parameters:");
    log::info!("Desired orbital altitude: [{}m]", TARGET_ALTITUDE);
    log::info!("Start of Gravity turn: [{}m]", TURN_START_ALT);
    log::info!("End of Gravity turn: [{}m]", TURN_END_ALT);

    let vessel = client.mk_call(&space_center::get_active_vessel())?;
    log::debug!("Active vessel: {:?}", vessel);

    let orbital_frame = client.mk_call(&vessel.get_orbital_reference_frame())?;

    // 1. Pre-Launch Set up
    // Initialize Control-Plane
    let (control, flight, orbit, auto_pilot, srb_resources) = batch_call!(
        &client,
        (
            &vessel.get_control(),
            &vessel.flight(&orbital_frame),
            &vessel.get_orbit(),
            &vessel.get_auto_pilot(),
            &vessel.resources_in_decouple_stage(2, true)
        )
    )?;

    let control = control?;
    let flight = flight?;
    let orbit = orbit?;
    let auto_pilot = auto_pilot?;
    let srb_resources = srb_resources?;

    // Start Telemetry
    let (alt_stream_handle, apoapsis_stream_handle, ut_stream_handle, srb_fuel_stream) = batch_call!(
        &client,
        (
            &flight.get_mean_altitude().to_stream(),
            &orbit.get_apoapsis_altitude().to_stream(),
            &space_center::get_ut().to_stream(),
            &srb_resources.amount(SRB_FUEL.to_string()).to_stream()
        )
    )?;

    let alt_stream_handle = alt_stream_handle?;
    let apoapsis_stream_handle = apoapsis_stream_handle?;
    let ut_stream_handle = ut_stream_handle?;
    let srb_fuel_stream = srb_fuel_stream?;

    // Prepare to launch
    let _ = batch_call!(
        &client,
        (
            &control.set_sas(false),    // Disable SAS
            &control.set_rcs(false),    // Disable RCS
            &control.set_throttle(1.0), // Max Throttle
        )
    )?;

    // 2. Launch.
    log::info!("Pre-flight checks completed. Starting countdown.");
    countdown(10);

    log::debug!("Activating next stage");
    log::debug!("Engaging Auto Pilot");
    log::debug!("Setting target pitch & headting [90, 90]");
    let _ = batch_call!(
        &client,
        (
            &control.activate_next_stage(),                 // Next Stage
            &auto_pilot.engage(),                           // Engage Auto-pilot
            &auto_pilot.target_pitch_and_heading(90., 90.)  // Set Pitch and heading (90, 90)
        )
    )?;

    // 3. Main Ascent loop
    let mut turn_angle = 0.0;
    let mut srb_seperated = false;
    let mut srb_fuel_seen_valid = false;
    loop {
        let update = match get_telemetry_update(
            &stream_client,
            (
                &ut_stream_handle,
                &apoapsis_stream_handle,
                &alt_stream_handle,
                &srb_fuel_stream,
            ),
        ) {
            Ok(update) => update,
            Err(why) => {
                log::warn!("Failed to get telemetry update: {}", why);
                continue;
            }
        };

        if let (_, Ok(apoapsis), Ok(altitude), srb_fuel) = update {
            // Gravity turn
            if altitude > TURN_START_ALT && altitude < TURN_END_ALT {
                log::trace!("Gravity Turn Tick");
                let frac = (altitude - TURN_START_ALT) / (TURN_END_ALT - TURN_START_ALT);

                let new_turn_angle = frac * 90.;

                if (new_turn_angle - turn_angle).abs() > 0.5 {
                    turn_angle = new_turn_angle;
                    client.mk_call(
                        &auto_pilot.target_pitch_and_heading(90. - turn_angle as f32, 90.),
                    )?;
                }
            }

            if let Ok(srb_fuel) = srb_fuel {
                // SRB Booster Seperation
                if !srb_seperated {
                    log::trace!("SRB Fuel: [{:?}]", srb_fuel);
                    if !srb_fuel_seen_valid && srb_fuel > 0.0 {
                        srb_fuel_seen_valid = true;
                    }

                    if srb_fuel <= 0.00 && srb_fuel_seen_valid {
                        log::info!("Detaching SRBs.");
                        client.mk_call(&control.activate_next_stage())?;
                        srb_seperated = true;
                        log::info!("SRB Seperation confirmed.");
                    }
                }
            }

            // Decrease throttle when approaching target apoapsis
            if apoapsis >= TARGET_ALTITUDE * 0.9 {
                log::info!("Aproaching target apoapsis [{}m]", TARGET_ALTITUDE);
                break;
            }
        }
    }

    // 4. Fine tune apoapsis
    log::debug!("Lowering throttle to [25%]");
    client.mk_call(&control.set_throttle(0.25))?; // 25% Throttle

    loop {
        let update = match get_telemetry_update(
            &stream_client,
            (
                &ut_stream_handle,
                &apoapsis_stream_handle,
                &alt_stream_handle,
                &srb_fuel_stream
            ),
        ) {
            Ok(update) => update,
            Err(_why) => {
                continue;
            }
        };

        if let (_, Ok(apoapsis), _, _) = update {
            if apoapsis >= TARGET_ALTITUDE {
                break;
            }
        }
    }

    log::info!("Target apoapsis reached.");
    log::debug!("Lowering throttle to [0%]");
    client.mk_call(&control.set_throttle(0.0))?; // Cut Throttle

    // 5. Coast out of atmosphere
    log::info!("Coasting out of atmosphere.");
    loop {
        let update = match get_telemetry_update(
            &stream_client,
            (
                &ut_stream_handle,
                &apoapsis_stream_handle,
                &alt_stream_handle,
                &srb_fuel_stream,
            ),
        ) {
            Ok(update) => update,
            Err(_why) => {
                continue;
            }
        };

        if let (_, _, Ok(altitude), _) = update {
            if altitude >= 70500. {
                break;
            }
        }
    }

    // 6. Plan circularization burn (using vis-viva equation)
    log::info!("Planning circularization burn");
    let body = client.mk_call(&orbit.get_body())?;
    let (mu, a2, a1) = batch_call!(
        &client,
        (
            &body.get_gravitational_parameter(),
            &orbit.get_apoapsis(),
            &orbit.get_semi_major_axis()
        )
    )?;

    let mu = mu? as f64;
    let a2 = a2?;
    let a1 = a1?;
    let v1 = (mu * ((2. / a2) - (1. / a1))).sqrt();
    let v2 = (mu * ((2. / a2) - (1. / a2))).sqrt();
    let delta_v = (v2 - v1) as f32;

    let (ut, time_to_apoapsis) = batch_call!(
        &client,
        (&space_center::get_ut(), &orbit.get_time_to_apoapsis())
    )?;

    // Create maneuver node.
    log::debug!("Creating maneuver node.");
    let node = client.mk_call(&control.add_node(ut? + time_to_apoapsis?, delta_v, 0., 0.))?;

    // Calculate burn time (using rocket equation)
    let (f, isp, m0) = batch_call!(
        &client,
        (
            &vessel.get_available_thrust(),
            &vessel.get_specific_impulse(),
            &vessel.get_mass()
        )
    )?;

    let f = f?;
    let isp = isp? * 9.82;
    let m0 = m0?;

    let m1 = m0 / (delta_v / isp).exp();
    let flow_rate = f / isp;
    let burn_time = (m0 - m1) / flow_rate;

    // Orientate ship
    log::info!("Orientating ship for circularization burn");

    log::info!("Getting reference frame.");
    let node_reference_frame = client.mk_call(&node.get_reference_frame())?;
    log::debug!("Reference Frame: {:?}", node_reference_frame);

    log::debug!("Setting reference frame");
    client.mk_call(&auto_pilot.set_reference_frame(&node_reference_frame))?;

    log::debug!("Getting directional vector");
    let (pitch, heading, roll) = client.mk_call(&flight.get_prograde())?;
    log::debug!("Directional Vector: ({}, {}, {})", pitch, heading, roll);


    log::debug!("Setting target direction");
    // client.mk_call(&auto_pilot.set_target_direction((pitch, heading, roll)))?;
    let _ = batch_call!(&client, (
            &auto_pilot.set_target_pitch(pitch as f32),
            &auto_pilot.set_target_heading(heading as f32)
    ))?;

    log::debug!("Waiting until oriented.");
    client.mk_call(&auto_pilot.wait())?;
    // let direction_offset_stream = client.mk_call(&auto_pilot.get_error().to_stream())?;
    // loop {
    //     let update = match stream_client.recv_update() {
    //         Ok(update) => update,
    //         Err(_) => continue
    //     };

    //     if let Ok(error) = update.get_result(&direction_offset_stream) {
    //         if error <= 1. {
    //             log::debug!("Oriented. Offset: [{:?}]", error);
    //             break;
    //         }
    //     }
    // }

    // Wait until burn
    log::info!("Waiting until circulization burn");
    let (ut, time_to_apoapsis) = batch_call!(
        &client,
        (&space_center::get_ut(), &orbit.get_time_to_apoapsis())
    )?;
    let burn_ut = ut? + time_to_apoapsis? - (burn_time / 2.) as f64;
    let lead_time = 5.;
    log::debug!("Warping...");
    client.mk_call(&space_center::warp_to(burn_ut - lead_time, 50., 4.))?;

    // Execute burn
    log::info!("Ready to execute burn");
    let tta_stream_handle = client.mk_call(&orbit.get_time_to_apoapsis().to_stream())?;

    loop {
        let update = match stream_client.recv_update() {
            Ok(update) => update,
            Err(_) => continue,
        };

        if let Ok(tta) = update.get_result(&tta_stream_handle) {
            if tta - (burn_time / 2.) as f64 <= 0. {
                break;
            }
        }
    }

    log::info!("Executing burn");
    client.mk_call(&control.set_throttle(1.0))?; // 100% Throttle

    log::debug!("Sleeping for [{}] seconds.", burn_time);
    std::thread::sleep(std::time::Duration::from_secs_f32(burn_time - 0.5));

    // println!("Fine tuning");
    // client.mk_call(&control.set_throttle(0.05))?; // 5% Throttle

    log::info!("Launch complete!");

    Ok(())
}

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type StreamResult<T> = std::result::Result<T, krpc_mars::error::Error>;

fn get_telemetry_update(
    stream_client: &krpc_mars::StreamClient,
    handles: (
        &StreamHandle<f64>,
        &StreamHandle<f64>,
        &StreamHandle<f64>,
        &StreamHandle<f32>,
    ),
) -> Result<(
    StreamResult<f64>,
    StreamResult<f64>,
    StreamResult<f64>,
    StreamResult<f32>,
)> {
    let update = stream_client.recv_update()?;

    let (ut_stream_handle, apoapsis_stream_handle, alt_stream_handle, srb_fuel_stream) = handles;

    let ut_result = update.get_result(&ut_stream_handle);
    let apoapsis_result = update.get_result(&apoapsis_stream_handle);
    let altitude_result = update.get_result(&alt_stream_handle);
    let srb_fuel = update.get_result(&srb_fuel_stream);

    Ok((ut_result, apoapsis_result, altitude_result, srb_fuel))
}
