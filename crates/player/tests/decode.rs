use std::time::Duration;

use player::Decoder;

fn dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn reference(name: &str) -> Vec<i32> {
    std::fs::read(dir().join(format!("{name}.raw")))
        .unwrap()
        .chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn open(name: &str) -> Decoder {
    let f = std::fs::File::open(dir().join(format!("{name}.mp4"))).unwrap();
    Decoder::new(Box::new(f)).unwrap()
}

fn drain(d: &mut Decoder) -> Vec<i32> {
    let mut out = Vec::new();
    while let Some(p) = d.next().unwrap() {
        assert_eq!(
            (p.rate, p.channels, p.bits),
            (d.rate(), d.channels(), d.bits())
        );
        out.extend(p.samples);
    }
    out
}

fn check(name: &str, rate: u32, bits: u16) {
    let mut d = open(name);
    assert_eq!((d.rate(), d.channels(), d.bits()), (rate, 2, bits));
    assert!(drain(&mut d) == reference(name), "{name} not bit-exact");
}

#[test]
fn decodes_16bit_bit_exact() {
    check("s16_44100", 44100, 16);
}

#[test]
fn decodes_24bit_bit_exact() {
    check("s24_96000", 96000, 24);
}

#[test]
fn seek_lands_near_target() {
    let mut d = open("s24_96000");
    let got = d.seek(Duration::from_millis(250)).unwrap();
    let diff = got.abs_diff(Duration::from_millis(250));
    assert!(diff <= Duration::from_millis(10), "landed at {got:?}");
    let frame = (got.as_secs_f64() * 96000.0).round() as usize;
    let rest = drain(&mut d);
    let raw = reference("s24_96000");
    assert!(!rest.is_empty());
    assert!(
        rest[..] == raw[frame * 2..frame * 2 + rest.len()],
        "post-seek samples differ at frame {frame}"
    );
}
