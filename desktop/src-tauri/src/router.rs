/// Determines whether a user message should be routed to the local LLM
/// (for image/download tasks) or to Claude (for everything else).
pub fn should_use_local(message: &str) -> bool {
    let lower = message.to_lowercase();

    // Image-related keywords
    let image_keywords = [
        "image", "images", "photo", "photos", "picture", "pictures",
        "download image", "find image", "scrape image", "get image",
        "download photo", "find photo", "scrape photo", "get photo",
        "download picture", "find picture",
        "wallpaper", "screenshot",
        "portrait", "headshot",
    ];

    // Action + subject patterns
    let download_patterns = [
        "download", "scrape", "grab", "fetch", "save image", "save photo",
    ];

    let media_context = [
        "jpg", "jpeg", "png", "webp", "gif",
        "thumbnail", "gallery", "album",
    ];

    // Direct match on image keywords
    for kw in &image_keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    // Download action + any visual context
    for dp in &download_patterns {
        if lower.contains(dp) {
            for mc in &media_context {
                if lower.contains(mc) {
                    return true;
                }
            }
        }
    }

    // Explicit local routing prefix
    if lower.starts_with("!local ") || lower.starts_with("/local ") {
        return true;
    }

    false
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
