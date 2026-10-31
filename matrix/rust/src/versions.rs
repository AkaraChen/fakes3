//! SDK versions, taken from Cargo.lock at compile time.
const LOCK: &str = include_str!("../Cargo.lock");

pub fn lock_version(name: &str) -> String {
    let needle = format!("name = \"{name}\"\nversion = \"");
    LOCK.find(&needle)
        .map(|i| {
            let rest = &LOCK[i + needle.len()..];
            rest[..rest.find('"').unwrap_or(0)].to_string()
        })
        .unwrap_or_else(|| "unknown".into())
}
