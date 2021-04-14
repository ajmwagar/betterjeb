use betterjeb::util::countdown;
use betterjeb::*;

use krpc_mars::{batch_call, batch_call_common, batch_call_unwrap};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let client = krpc_mars::RPCClient::connect("Example", "192.168.0.40:50000")?;
    let stream_client = krpc_mars::StreamClient::connect(&client, "192.168.0.40:50001")?;

    let (vessel, ut_stream_handle, warp_factor_handle) = batch_call_unwrap!(&client, (
	    &space_center::get_active_vessel(),
	    &space_center::get_ut().to_stream(),
	    &space_center::get_warp_factor().to_stream(),
    ))?;

    loop {
	let update = stream_client.recv_update()?;

        println!("Got Stream Update: {:?}", update);

        if let Ok(ut_result) = update.get_result(&ut_stream_handle) {
            println!("ut: {}", ut_result);
        }
        if let Ok(warp) = update.get_result(&warp_factor_handle) {
	  println!("warp: {:?}", warp);
        }
    }

    Ok(())
}
