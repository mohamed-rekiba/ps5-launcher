//! assets/emulators.schema.json, generated from the manifest's Rust types, so editors check
//! assets/emulators.yaml with the same rules as the launcher. The Rust validation stays the
//! authority: the schema cannot say, for example, that a default names an existing emulator.
//!
//! When the types change, refresh the file with:
//!
//!     cargo test --locked emulators::schema::tests::write -- --ignored

use super::manifest::Manifest;

pub const REFRESH: &str = "cargo test --locked emulators::schema::tests::write -- --ignored";

/// The schema as committed: pretty JSON with a final newline.
pub fn generate() -> String {
    let schema = schemars::schema_for!(Manifest);
    let mut text = serde_json::to_string_pretty(&schema).expect("a schema is JSON");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/emulators.schema.json");

    #[test]
    fn matches_the_committed_file() {
        // Compared as JSON values: key order can differ between builds (serde_json's
        // preserve_order feature depends on the platform's dependencies).
        let committed: Option<serde_json::Value> = std::fs::read_to_string(PATH).ok().and_then(|t| serde_json::from_str(&t).ok());
        let generated: serde_json::Value = serde_json::from_str(&generate()).unwrap();
        assert!(committed.as_ref() == Some(&generated), "assets/emulators.schema.json is out of date. Refresh it with:\n\n    {REFRESH}\n");
    }

    #[test]
    fn refuses_unknown_fields() {
        let schema: serde_json::Value = serde_json::from_str(&generate()).unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["$defs"]["Emulator"]["additionalProperties"], false);
    }

    #[test]
    fn the_manifest_names_it_for_editors() {
        assert!(super::super::registry::EMBEDDED.starts_with("# yaml-language-server: $schema=./emulators.schema.json\n"));
    }

    #[test]
    #[ignore = "writes assets/emulators.schema.json; run it after changing the manifest's types"]
    fn write() {
        std::fs::write(PATH, generate()).unwrap();
    }
}
