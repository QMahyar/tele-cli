pub const PAGE_ITEMS: usize = 100;

pub fn needs_page_token(served: usize) -> bool {
    served > 0 && served.is_multiple_of(PAGE_ITEMS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_page_token_fires_on_page_boundaries_only() {
        assert!(!needs_page_token(0));
        assert!(!needs_page_token(1));
        assert!(!needs_page_token(99));
        assert!(needs_page_token(100));
        assert!(!needs_page_token(101));
        assert!(needs_page_token(200));
    }
}
