/// Generates a fake BUVID3 identifier for Bilibili API requests.
///
/// BUVID3 is a unique identifier used by Bilibili for tracking and authentication.
/// This function creates a fake one by generating a UUID, removing hyphens,
/// converting to uppercase, and formatting it with the required pattern ending in "infoc".
///
/// # Returns
///
/// A string in the format `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXXinfoc` where X are hexadecimal characters.
pub fn generate_fake_buvid3() -> String {
    let u = uuid::Uuid::new_v4();
    let u_str = u.to_string().to_uppercase().replace('-', "");
    format!(
        "{}-{}-{}-{}-{}infoc",
        &u_str[0..8],
        &u_str[8..12],
        &u_str[12..16],
        &u_str[16..20],
        &u_str[20..]
    )
}
