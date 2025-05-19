fn main() {
	println!(
		"cargo:rustc-env=GIT_COMMIT_HASH={}",
		env!("GIT_COMMIT_HASH")
	);
}
