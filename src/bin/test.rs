use betterjeb::*;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let client = krpc_mars::RPCClient::connect("Example", "192.168.0.40:50000")?;

    let vessel = client.mk_call(&space_center::get_active_vessel())?;
    println!("Active vessel: {:?}", vessel);

    let crew = client.mk_call(&vessel.get_crew())?;
    println!("Crew: {:?}", crew);

    Ok(())
}
