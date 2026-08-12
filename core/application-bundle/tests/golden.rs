use hoplite_application_bundle::{decode, encode, Error, FORMAT, HARA_BUNDLE_MAGIC, MAGIC};

const MANIFEST: &[u8] = b"golden-route-manifest";
const BYTECODE: &[u8] = b"HBX0golden-bytecode";

fn fixture_hex(input: &str) -> Vec<u8> {
    let compact = input
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::chars)
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();

    assert_eq!(compact.len() % 2, 0, "fixture must contain complete bytes");
    (0..compact.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&compact[offset..offset + 2], 16)
                .expect("fixture must contain lowercase hexadecimal bytes")
        })
        .collect()
}

#[test]
fn committed_hab0_bytes_are_the_encoder_contract() {
    let golden = fixture_hex(include_str!("fixtures/hab0-golden.hex"));

    assert_eq!(FORMAT, "hoplite.application-bundle/0-alpha");
    assert_eq!(&golden[..MAGIC.len()], MAGIC);
    assert_eq!(golden.len(), 95);
    assert_eq!(encode(MANIFEST, BYTECODE).unwrap(), golden);

    let decoded = decode(&golden, MANIFEST).unwrap();
    assert_eq!(decoded.bytecode(), BYTECODE);
    assert_eq!(&decoded.bytecode()[..HARA_BUNDLE_MAGIC.len()], HARA_BUNDLE_MAGIC);
}

#[test]
fn reserved_next_epoch_requires_an_explicit_migration() {
    let current = fixture_hex(include_str!("fixtures/hab0-golden.hex"));
    let next_epoch = fixture_hex(include_str!("fixtures/hab1-migration-rejected.hex"));
    let mut reserved_marker = *MAGIC;
    reserved_marker[3] = b'1';

    assert_eq!(&next_epoch[..MAGIC.len()], &reserved_marker);
    assert_eq!(&next_epoch[MAGIC.len()..], &current[MAGIC.len()..]);
    assert_eq!(decode(&next_epoch, MANIFEST), Err(Error::InvalidMagic));
}
