use super::*;
#[test]
fn extensions_survive_typed_rewrites_without_restoring_deleted_items() {
    let original = json!({"schema_extension":{"version":2},"tasks":[{"id":"a","known":1,"extension":{"retained":true}},{"id":"deleted","extension":true}]});
    let known = json!({"tasks":[{"id":"a","known":1},{"id":"deleted"}]});
    let mut updated = json!({"tasks":[{"id":"new","known":3},{"id":"a","known":2}]});
    retain_extensions(&original, &known, &mut updated);
    assert_eq!(updated["schema_extension"]["version"], 2);
    assert_eq!(updated["tasks"][1]["extension"]["retained"], true);
    assert_eq!(updated["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(updated["tasks"][1]["known"], 2);
    assert!(canonical_json("people.json", b"{broken").is_err());
    assert!(canonical_json("tags.json", br#"{"tags":[{"id":4}]}"#).is_err());
}
