use crate::screen::face_64x128::SplashContent;
use crate::screen::render::layout::{FONT_6X10_CHAR_W, SPLASH_TEXT_X, WIDTH};

#[test]
fn every_splash_line_fits_its_rendered_font() {
    let max_chars = ((WIDTH - SPLASH_TEXT_X) / FONT_6X10_CHAR_W) as usize;
    for content in SplashContent::ALL {
        for line in content.lines() {
            assert!(
                line.chars().count() <= max_chars,
                "{content:?} line {line:?} is {} characters, more than the {max_chars} that fit",
                line.chars().count()
            );
        }
    }
}

#[test]
fn the_brand_splash_still_says_the_whole_name() {
    let joined = SplashContent::Brand.lines().join(" ");
    assert_eq!(joined, "Personal Hopspot");
}
