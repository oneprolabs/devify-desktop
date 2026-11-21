use super::{pick_personal_space, DevifySpace};

fn space(id: &str, name: &str, kind: &str) -> DevifySpace {
    DevifySpace {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        status: "active".into(),
        role: None,
    }
}

#[test]
fn personal_space_is_the_default_target() {
    let spaces = vec![
        space("1", "团队空间", "team"),
        space("2", "个人空间", "personal"),
    ];
    assert_eq!(pick_personal_space(&spaces), Some("2".into()));
}

#[test]
fn falls_back_to_name_then_first_space() {
    let named = vec![
        space("3", "团队", "team"),
        space("4", "我的个人空间", "shared"),
    ];
    assert_eq!(pick_personal_space(&named), Some("4".into()));
    let unnamed = vec![space("7", "Alpha", "team"), space("8", "Beta", "team")];
    assert_eq!(pick_personal_space(&unnamed), Some("7".into()));
    assert_eq!(pick_personal_space(&[]), None);
}
