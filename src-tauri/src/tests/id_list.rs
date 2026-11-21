use super::id_list;

#[test]
fn accepts_arrays_and_json_strings() {
    let array = serde_json::json!([22, 23]);
    assert_eq!(id_list(Some(&array)), vec![22, 23]);
    let text = serde_json::json!("[22, 23]");
    assert_eq!(id_list(Some(&text)), vec![22, 23]);
    let strings = serde_json::json!("[\"22\"]");
    assert_eq!(id_list(Some(&strings)), vec![22]);
    let empty = serde_json::json!("[]");
    assert!(id_list(Some(&empty)).is_empty());
    assert!(id_list(None).is_empty());
    assert!(id_list(Some(&serde_json::json!("not json"))).is_empty());
}
