use deepika_sync::discovery::service_name;

#[test]
fn service_name_fits_an_mdns_label_and_separates_sessions() {
    let a = service_name("capability-a");
    assert!(a.len() <= 15 && a.starts_with("sp-"));
    assert_eq!(a, service_name("capability-a"));
    assert_ne!(a, service_name("capability-b"));
}
