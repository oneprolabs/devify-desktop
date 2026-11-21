use super::*;
#[test]
fn extension_keeps_only_safe_suffixes() {
    assert_eq!(attachment_extension(Some("report.pdf")), "pdf");
    assert_eq!(attachment_extension(Some("HOLIDAY.JPG")), "jpg");
    assert_eq!(attachment_extension(Some("archive.tar.gz")), "gz");
}

#[test]
fn extension_falls_back_when_missing_or_unsafe() {
    assert_eq!(attachment_extension(None), "bin");
    assert_eq!(attachment_extension(Some("no-extension")), "bin");
    assert_eq!(attachment_extension(Some("../../etc/passwd")), "bin");
    assert_eq!(attachment_extension(Some("name.verylongextension")), "bin");
    assert_eq!(attachment_extension(Some("name.we ird")), "bin");
}
