/// All messages are routed to the local LLM (LM Studio).
/// Claude/Anthropic API is no longer used.
pub fn should_use_local(_message: &str) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing() {
        assert!(should_use_local("Download 10 images of cats"));
        assert!(should_use_local("Find photos of Sydney Sweeney"));
        assert!(should_use_local("get me a portrait of Obama"));
        assert!(should_use_local("!local write me a poem"));
        assert!(!should_use_local("Write a Python script"));
        assert!(!should_use_local("Summarize this page"));
        assert!(!should_use_local("What is the capital of France?"));
    }
}
