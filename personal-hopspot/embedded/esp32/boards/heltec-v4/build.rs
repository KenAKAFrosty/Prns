#[path = "../s3_build.rs"]
mod s3_build;

fn main() {
    s3_build::link_memory_layout();
}
