//! Keeps `spec/PROFILE.md` honest: its machine-checked Limits table must list
//! exactly the fields and values of `Limits::default()`. (`cargo xtask
//! preflight` checks the same table against the source text, so a mismatch
//! fails fast even before tests run.)

use rfb_proto::Limits;
use std::collections::BTreeMap;

fn spec_table() -> BTreeMap<String, u64> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/PROFILE.md");
    let text = std::fs::read_to_string(path).expect("spec/PROFILE.md is readable");
    let start = text.find("<!-- BEGIN LIMITS").expect("PROFILE.md has a BEGIN LIMITS marker");
    let end = text.find("<!-- END LIMITS -->").expect("PROFILE.md has an END LIMITS marker");
    let mut rows = BTreeMap::new();
    for line in text[start..end].lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // | `field` | value | description |  ->  ["", "`field`", "value", "description", ""]
        if cells.len() < 4 || !cells[1].starts_with('`') {
            continue;
        }
        let name = cells[1].trim_matches('`').to_string();
        let value = cells[2].parse().unwrap_or_else(|_| panic!("Limits row {name}: {:?} is not decimal", cells[2]));
        assert!(rows.insert(name.clone(), value).is_none(), "Limits row {name} appears twice");
    }
    rows
}

#[test]
fn spec_limits_table_matches_default() {
    // Exhaustive destructuring: adding a field to Limits stops this compiling
    // until the spec (and this test) are updated.
    let Limits {
        max_name_len,
        max_reason_len,
        max_cut_text_len,
        max_encodings,
        max_colour_map_entries,
        max_vencrypt_subtypes,
        max_credential_len,
        max_cursor_size,
        max_aurora_msg_len,
        max_video_tiles,
        max_video_payload,
    } = Limits::default();
    let actual: BTreeMap<String, u64> = [
        ("max_name_len", u64::from(max_name_len)),
        ("max_reason_len", u64::from(max_reason_len)),
        ("max_cut_text_len", u64::from(max_cut_text_len)),
        ("max_encodings", u64::from(max_encodings)),
        ("max_colour_map_entries", u64::from(max_colour_map_entries)),
        ("max_vencrypt_subtypes", u64::from(max_vencrypt_subtypes)),
        ("max_credential_len", u64::from(max_credential_len)),
        ("max_cursor_size", u64::from(max_cursor_size)),
        ("max_aurora_msg_len", u64::from(max_aurora_msg_len)),
        ("max_video_tiles", u64::from(max_video_tiles)),
        ("max_video_payload", u64::from(max_video_payload)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(spec_table(), actual, "spec/PROFILE.md section 7 is out of date with Limits::default()");
}
