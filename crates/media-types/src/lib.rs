/// Video resolution information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    #[inline]
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_display() {
        let r = Resolution::new(1920, 1080);
        assert_eq!(r.to_string(), "1920x1080");
    }

    #[test]
    fn test_resolution_equality() {
        assert_eq!(Resolution::new(1, 2), Resolution::new(1, 2));
        assert_ne!(Resolution::new(1, 2), Resolution::new(2, 1));
    }
}
