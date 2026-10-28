mod support;

use psx_cd::Image;
use std::fs;

fn assert_extracts_fixture(raw: bool, test: &str) {
    let dir = support::out_dir(test);
    let root = dir;
    let image_path = root.join(if raw { "fixture.bin" } else { "fixture.iso" });
    fs::write(&image_path, support::fixture(raw, false)).unwrap();
    let paths = Image::open(&image_path).unwrap().extract(root.join("out")).unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(fs::read(root.join("out/HELLO.TXT")).unwrap(), b"hello");
    assert_eq!(fs::read(root.join("out/DIR/DATA.BIN")).unwrap(), [0, 1, 255]);
}

#[test]
fn extracts_cooked_iso_fixture_byte_exactly() {
    assert_extracts_fixture(false, "extracts_cooked_iso_fixture_byte_exactly");
}

#[test]
fn extracts_raw_mode2_fixture_byte_exactly() {
    let dir = support::out_dir("extracts_raw_mode2_fixture_byte_exactly");
    let root = dir;
    let image_path = root.join("fixture.bin");
    fs::write(&image_path, support::fixture(true, false)).unwrap();
    Image::open(&image_path).unwrap().extract(root.join("out")).unwrap();
    assert_eq!(fs::read(root.join("out/HELLO.TXT")).unwrap(), b"hello");
    assert_eq!(fs::metadata(root.join("out/license_data.dat")).unwrap().len(), 28_032);
}
