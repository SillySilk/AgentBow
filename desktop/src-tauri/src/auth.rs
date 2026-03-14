pub fn validate_token(provided: &str, expected: &str) -> bool {
    // Constant-time comparison to prevent timing attacks
    if provided.len() != expected.len() {
        return false;
    }
    provided
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}
