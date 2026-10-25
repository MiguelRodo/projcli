//! Typed R settings remain explicit and backwards compatible.
use proj::yml::{ProjConfig,RSession};

#[test]
fn r_defaults_are_isolated_and_path_selected_only_when_explicit() {
    let config: ProjConfig=serde_json::from_str("{}").unwrap();
    assert_eq!(config.config.r.session,RSession::Isolated);
    assert!(config.config.r.rscript.is_none());
    let config: ProjConfig=serde_json::from_str(r#"{"config":{"r":{"session":"shared","rscript":"R installation/Rscript"}}}"#).unwrap();
    assert_eq!(config.config.r.session,RSession::Shared);
    assert_eq!(config.config.r.rscript.unwrap().as_str(),"R installation/Rscript");
}

#[test]
fn r_configuration_rejects_unknown_modes_keys_and_nonstring_paths() {
    for text in [
        r#"{"config":{"r":{"session":"automatic"}}}"#,
        r#"{"config":{"r":{"shared":true}}}"#,
        r#"{"config":{"r":{"rscript":["Rscript","--vanilla"]}}}"#,
    ] {assert!(serde_json::from_str::<ProjConfig>(text).is_err());}
}
