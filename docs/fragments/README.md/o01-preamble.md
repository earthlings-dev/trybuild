Trybuild
========

[<img alt="github" src="https://img.shields.io/badge/github-dtolnay/trybuild-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/dtolnay/trybuild)
[<img alt="crates.io" src="https://img.shields.io/crates/v/trybuild.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/trybuild)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-trybuild-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://docs.rs/trybuild)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/dtolnay/trybuild/ci.yml?branch=master&style=for-the-badge" height="20">](https://github.com/dtolnay/trybuild/actions?query=branch%3Amaster)

Trybuild is a test harness for invoking rustc on a set of test cases and
asserting that any resulting error messages are the ones intended.

<p align="center">
<a href="#compile-fail-tests">
<img src="https://user-images.githubusercontent.com/1940490/57186574-76469e00-6e96-11e9-8cb5-b63b657170c9.png" width="600">
</a>
</p>

Such tests are commonly useful for testing error reporting involving procedural
macros. We would write test cases triggering either errors detected by the macro
or errors detected by the Rust compiler in the resulting expanded code, and
compare against the expected errors to ensure that they remain user-friendly.

This style of testing is sometimes called *ui tests* because they test aspects
of the user's interaction with a library outside of what would be covered by
ordinary API tests.

Nothing here is specific to macros; trybuild would work equally well for testing
misuse of non-macro APIs.

```toml
[dev-dependencies]
trybuild = "1.0"
```

<br>
