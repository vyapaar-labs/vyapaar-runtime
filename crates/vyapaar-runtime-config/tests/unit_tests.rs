use vyapaar_core::{Exchange, InstrumentId};
use vyapaar_runtime_config::ResolvedUniverse;

#[test]
fn resolved_universe_holds_instruments() {
    let inst = InstrumentId::new(Exchange::Nse, "INFY");
    let universe = ResolvedUniverse::new("test", vec![inst.clone()]);
    assert_eq!(universe.instruments, vec![inst]);
}
