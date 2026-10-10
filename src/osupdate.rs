//! PS5 Launcher OS's own state from `bootc status --json`: what runs now, whether an update waits
//! for the next restart, and whether there is a system to roll back to (Phase 6 of
//! docs/plans/ps5-launcher-os.md). Only parsing lives here; the root helper runs bootc.
#![allow(dead_code)] // nothing calls it until the Updates page (Phase 6)

/// One system image on the PC.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deployment {
    /// The image name with its tag, for example "ghcr.io/owner/ps5-launcher-os:main".
    pub image: String,
    pub digest: String,
    /// The image's version label, for example "44.20261010.0".
    pub version: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    pub booted: Deployment,
    /// Downloaded and installed at the next restart ("Update and restart").
    pub staged: Option<Deployment>,
    /// The previous system, which "Undo the last system update" starts.
    pub rollback: Option<Deployment>,
    /// The version `bootc upgrade --check` found, not downloaded yet.
    pub available: Option<String>,
}

/// Parse `bootc status --json`. On a terminal bootc prints a progress line first, so the JSON
/// starts at the first `{`.
pub fn parse_status(output: &str) -> Result<Status, String> {
    let json = output.find('{').map(|start| &output[start..]).ok_or("no JSON in bootc's output")?;
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unexpected bootc status: {e}"))?;
    let status = &v["status"];
    let booted = deployment(&status["booted"]).ok_or("bootc status has no booted system")?;
    Ok(Status {
        booted,
        staged: deployment(&status["staged"]),
        rollback: deployment(&status["rollback"]),
        available: status["booted"]["cachedUpdate"]["version"].as_str().map(String::from),
    })
}

/// A deployment entry (`booted`, `staged` or `rollback`), or None when it is null or incomplete.
fn deployment(v: &serde_json::Value) -> Option<Deployment> {
    let image = &v["image"];
    Some(Deployment {
        image: image["image"]["image"].as_str()?.to_string(),
        digest: image["imageDigest"].as_str()?.to_string(),
        version: image["version"].as_str().map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOTED: &str = include_str!("testdata/fedora44-vm-bootc-status.txt");

    /// The captured status, with `field` of "status" replaced by `value` (same shape as `booted`).
    fn with(field: &str, value: serde_json::Value) -> String {
        let json = &BOOTED[BOOTED.find('{').unwrap()..];
        let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
        v["status"][field] = value;
        v.to_string()
    }

    fn booted() -> Deployment {
        Deployment {
            image: "localhost/ps5-launcher-os:boottest".into(),
            digest: "sha256:feda9c45f80a4bfe4541a133edbe69410aba4f0d03003fec7ddfdc3c7e7f20d6".into(),
            version: Some("44.20261010.0".into()),
        }
    }

    #[test]
    fn the_running_system_from_real_output() {
        assert_eq!(
            parse_status(BOOTED),
            Ok(Status { booted: booted(), staged: None, rollback: None, available: None })
        );
    }

    #[test]
    fn a_staged_update_and_a_rollback() {
        let json = &BOOTED[BOOTED.find('{').unwrap()..];
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut newer = v["status"]["booted"].clone();
        newer["image"]["imageDigest"] = "sha256:0123".into();
        newer["image"]["version"] = "44.20261011.0".into();
        let staged = parse_status(&with("staged", newer.clone())).unwrap().staged.unwrap();
        assert_eq!(staged.digest, "sha256:0123");
        assert_eq!(staged.version.as_deref(), Some("44.20261011.0"));
        assert_eq!(parse_status(&with("rollback", newer)).unwrap().rollback.unwrap().digest, "sha256:0123");
    }

    #[test]
    fn an_update_found_by_check() {
        let mut cached = serde_json::json!({});
        cached["version"] = "44.20261012.0".into();
        cached["imageDigest"] = "sha256:4567".into();
        let mut booted = serde_json::from_str::<serde_json::Value>(&BOOTED[BOOTED.find('{').unwrap()..]).unwrap();
        booted["status"]["booted"]["cachedUpdate"] = cached;
        assert_eq!(parse_status(&booted.to_string()).unwrap().available.as_deref(), Some("44.20261012.0"));
    }

    #[test]
    fn output_that_is_not_bootc_status_is_an_error() {
        assert!(parse_status("").is_err());
        assert!(parse_status("error: Not running on a bootc system").is_err());
        assert!(parse_status(r#"{"status": {}}"#).is_err(), "no booted system");
    }
}
