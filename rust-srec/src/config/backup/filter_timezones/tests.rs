use super::*;
use serde_json::json;

#[test]
fn legacy_and_current_bundles_keep_their_defaults_and_extensions() {
    for version in ["0.1.0", "0.1.7", EXPORT_SCHEMA_VERSION, "0.2.0"] {
        for null in [false, true] {
            let mut config = json!({"days_of_week":["Monday"], "start_time":"09:00", "end_time":"17:00", "extension":{"nested":[1,true,"kept"]}});
            if null {
                config["timezone"] = Value::Null;
            }
            for encoded in [
                config.clone(),
                Value::String(Value::String(config.to_string()).to_string()),
            ] {
                let imported = import_filter_config(version, "TIME_BASED", encoded);
                assert_eq!(imported["extension"], config["extension"]);
                let expected = if matches!(version, "0.1.0" | "0.1.7") {
                    "local"
                } else {
                    "UTC"
                };
                let exported = export_filter_config("TIME_BASED", imported);
                assert_eq!(exported["timezone"], expected);
                assert_eq!(
                    import_filter_config(EXPORT_SCHEMA_VERSION, "TIME_BASED", exported.clone()),
                    exported
                );
            }
            let cron = import_filter_config(version, "CRON", json!({"expression":"0 * * * * *"}));
            assert_eq!(export_filter_config("CRON", cron)["timezone"], "UTC");
        }
    }
}

#[test]
fn explicit_zones_unknown_members_and_invalid_shapes_are_not_reinterpreted() {
    for zone in ["UTC", "local", "Europe/Madrid", "invalid/zone", ""] {
        let config = json!({"timezone":zone, "extension": [1,2,3]});
        assert_eq!(
            import_filter_config("0.1.7", "TIME_BASED", config.clone()),
            config
        );
        assert_eq!(export_filter_config("TIME_BASED", config.clone()), config);
    }
    for config in [Value::Null, json!([]), json!(42), json!("malformed")] {
        assert_eq!(
            import_filter_config("0.1.7", "TIME_BASED", config.clone()),
            config
        );
        assert_eq!(export_filter_config("TIME_BASED", config.clone()), config);
    }
    let keyword = json!({"include":[], "exclude":[], "timezone":null});
    assert_eq!(
        import_filter_config("0.1.7", "KEYWORD", keyword.clone()),
        keyword
    );
    assert_eq!(export_filter_config("KEYWORD", keyword.clone()), keyword);
}
