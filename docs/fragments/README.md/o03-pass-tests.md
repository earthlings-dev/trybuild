## Pass tests

The same test harness is able to run tests that are expected to pass, too.
Ordinarily you would just have Cargo run such tests directly, but being able to
combine modes like this could be useful for workshops in which participants work
through test cases enabling one at a time. Trybuild was originally developed for
my [procedural macros workshop at Rust Latam][workshop].

[workshop]: https://github.com/dtolnay/proc-macro-workshop

```rust
#[test]
fn ui() {
    let mut t = trybuild::TestCases::new();
    t.pass("tests/01-parse-header.rs");
    t.pass("tests/02-parse-body.rs");
    t.compile_fail("tests/03-expand-four-errors.rs");
    t.pass("tests/04-paste-ident.rs");
    t.pass("tests/05-repeat-section.rs");
    //t.pass("tests/06-make-work-in-function.rs");
    //t.pass("tests/07-init-array.rs");
    //t.compile_fail("tests/08-ident-span.rs");
    t.run().unwrap();
}
```

Pass tests are considered to succeed if they compile successfully and have a
`main` function that does not panic when the compiled binary is executed.

<p align="center">
<a href="#pass-tests">
<img src="https://user-images.githubusercontent.com/1940490/57186580-7f376f80-6e96-11e9-9cae-8257609269ef.png" width="600">
</a>
</p>

<br>
