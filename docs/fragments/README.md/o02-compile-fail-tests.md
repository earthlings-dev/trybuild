## Compile-fail tests

A minimal trybuild setup looks like this:

```rust
#[test]
fn ui() {
    let mut t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    t.run().unwrap();
}
```

The test can be run with `cargo test`. It will individually compile each of the
source files matching the glob pattern, expect them to fail to compile, and
assert that the compiler's error message matches an adjacently named _*.stderr_
file containing the expected output (same file name as the test except with a
different extension). If it matches, the test case is considered to succeed.

Dependencies listed under `[dev-dependencies]` in the project's Cargo.toml are
accessible from within the test cases.

Failing tests display the expected vs actual compiler output inline.

<p align="center">
<a href="#compile-fail-tests">
<img src="https://user-images.githubusercontent.com/1940490/57186575-79418e80-6e96-11e9-9478-c9b3dc10327f.png" width="600">
</a>
</p>

A compile\_fail test that fails to fail to compile is also a failure.

<p align="center">
<a href="#compile-fail-tests">
<img src="https://user-images.githubusercontent.com/1940490/57186576-7b0b5200-6e96-11e9-8bfd-2de705125108.png" width="600">
</a>
</p>

To test just one source file, use:
```
cargo test -- ui trybuild=example.rs
```
where `ui` is the name of the `#[test]` function that invokes `trybuild`, and
`example.rs` is the name of the file to test.

<br>
