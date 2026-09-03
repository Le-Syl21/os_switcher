//! Tests of the BCD semantics on a synthetic multi-OS hive built
//! with regf-rs (no real machine data).

use os_switcher::bcd::{Bcd, BOOTMGR};
use regf_rs::{Hive, RegValue};

const WIN10: &str = "{aaaaaaaa-0000-0000-0000-000000000001}";
const WIN11: &str = "{aaaaaaaa-0000-0000-0000-000000000002}";
const UBUNTU: &str = "{aaaaaaaa-0000-0000-0000-000000000003}";

/// Builds a synthetic BCD: three OSes, Windows 10 as default.
fn synthetic_bcd_bytes() -> Vec<u8> {
    let mut h = Hive::new_empty("BCD");

    // Boot Manager: DefaultObject + DisplayOrder.
    let bm = format!("Objects\\{BOOTMGR}\\Elements");
    mkval(
        &mut h,
        &format!("{bm}\\23000003"),
        RegValue::Sz(WIN10.into()),
    );
    mkval(
        &mut h,
        &format!("{bm}\\24000001"),
        RegValue::MultiSz(vec![WIN10.into(), WIN11.into(), UBUNTU.into()]),
    );

    // Three OS loaders with a description.
    for (guid, desc) in [
        (WIN10, "Windows 10"),
        (WIN11, "Windows 11"),
        (UBUNTU, "Ubuntu"),
    ] {
        mkval(
            &mut h,
            &format!("Objects\\{guid}\\Elements\\12000004"),
            RegValue::Sz(desc.into()),
        );
    }
    h.to_bytes()
}

fn mkval(h: &mut Hive, path: &str, v: RegValue) {
    h.create_key(path).unwrap();
    h.set_value(path, "Element", v).unwrap();
}

#[test]
fn lists_entries_with_descriptions() {
    let bcd = Bcd::from_bytes(synthetic_bcd_bytes()).unwrap();
    let entries = bcd.entries();
    let names: Vec<_> = entries.iter().map(|e| e.description.as_str()).collect();
    assert_eq!(names, ["Windows 10", "Windows 11", "Ubuntu"]);
    assert_eq!(entries[0].id, WIN10);
}

#[test]
fn reads_default() {
    let bcd = Bcd::from_bytes(synthetic_bcd_bytes()).unwrap();
    assert_eq!(bcd.default().as_deref(), Some(WIN10));
}

#[test]
fn sets_default() {
    let mut bcd = Bcd::from_bytes(synthetic_bcd_bytes()).unwrap();
    bcd.set_default(WIN11).unwrap();
    assert_eq!(bcd.default().as_deref(), Some(WIN11));

    // Persiste après round-trip.
    let reloaded = Bcd::from_bytes(bcd.to_bytes()).unwrap();
    assert_eq!(reloaded.default().as_deref(), Some(WIN11));
}

#[test]
fn arms_and_clears_boot_sequence() {
    let mut bcd = Bcd::from_bytes(synthetic_bcd_bytes()).unwrap();
    assert!(bcd.boot_sequence().is_empty());

    bcd.set_boot_sequence(UBUNTU).unwrap();
    assert_eq!(bcd.boot_sequence(), vec![UBUNTU.to_string()]);

    // Persiste, puis s'annule.
    let mut reloaded = Bcd::from_bytes(bcd.to_bytes()).unwrap();
    assert_eq!(reloaded.boot_sequence(), vec![UBUNTU.to_string()]);
    reloaded.clear_boot_sequence().unwrap();
    assert!(reloaded.boot_sequence().is_empty());

    // clear est idempotent.
    reloaded.clear_boot_sequence().unwrap();
}

#[test]
fn rejects_non_bcd() {
    // A hive with no Boot Manager is not a BCD.
    let bytes = Hive::new_empty("NOTBCD").to_bytes();
    assert!(matches!(
        Bcd::from_bytes(bytes),
        Err(os_switcher::bcd::Error::NotABcd)
    ));
}
