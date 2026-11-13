//! Die Standardwerte der Entwicklungsparameter gibt es zweimal: hier in Rust und als
//! `DEFAULT_PARAMS` in `src/lib/api.ts`. Beide Seiten vergleichen sich mit derselben
//! Datei `default-params.json` (die Oberfläche in `tools/mirrors.test.mjs`).

use lumenkorn_develop::DevelopParams;

#[test]
fn defaults_match_the_shared_file() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("default-params.json")).unwrap();
    let actual = serde_json::to_value(DevelopParams::default()).unwrap();
    assert_eq!(
        actual,
        expected,
        "Standardwerte geändert: default-params.json und DEFAULT_PARAMS in api.ts anpassen:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}
