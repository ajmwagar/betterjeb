#[macro_use]
extern crate krpc_mars;

pub mod drawing;
pub mod infernal_robotics;
pub mod kerbal_alarm_clock;
pub mod remote_tech;
pub mod space_center;
pub mod ui;

pub mod util {
    /// Countdown that logs to console
    pub fn countdown(seconds: usize) {
        for second in 0..seconds {
            log::info!("T-{}...", seconds-second);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        log::info!("Ignition!")
    }

}
