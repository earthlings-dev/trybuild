## What to test

When it comes to compile-fail tests, write tests for anything for which you care
to find out when there are changes in the user-facing compiler output. As a
negative example, please don't write compile-fail tests simply calling all of
your public APIs with arguments of the wrong type; there would be no benefit.

A common use would be for testing specific targeted error messages emitted by a
procedural macro. For example the derive macro from the [`ref-cast`] crate is
required to be placed on a type that has either `#[repr(C)]` or
`#[repr(transparent)]` in order for the expansion to be free of undefined
behavior, which it enforces at compile time:

[`ref-cast`]: https://github.com/dtolnay/ref-cast

```console
error: RefCast trait requires #[repr(C)] or #[repr(transparent)]
 --> $DIR/missing-repr.rs:3:10
  |
3 | #[derive(RefCast)]
  |          ^^^^^^^
```

Macros that consume helper attributes will want to check that unrecognized
content within those attributes is properly indicated to the caller. Is the
error message correctly placed under the erroneous tokens, not on a useless
call\_site span?

```console
error: unknown serde field attribute `qqq`
 --> $DIR/unknown-attribute.rs:5:13
  |
5 |     #[serde(qqq = "...")]
  |             ^^^
```

Declarative macros can benefit from compile-fail tests too. The [`json!`] macro
from serde\_json is just a great big macro\_rules macro but makes an effort to
have error messages from broken JSON in the input always appear on the most
appropriate token:

[`json!`]: https://docs.rs/serde_json/1.0/serde_json/macro.json.html

```console
error: no rules expected the token `,`
 --> $DIR/double-comma.rs:4:38
  |
4 |     println!("{}", json!({ "k": null,, }));
  |                                      ^ no rules expected this token in macro call
```

Sometimes we may have a macro that expands successfully but we count on it to
trigger particular compiler errors at some point beyond macro expansion. For
example the [`readonly`] crate introduces struct fields that are public but
readable only, even if the caller has a &mut reference to the surrounding
struct. If someone writes to a readonly field, we need to be sure that it
wouldn't compile:

[`readonly`]: https://github.com/dtolnay/readonly

```console
error[E0594]: cannot assign to data in a `&` reference
  --> $DIR/write-a-readonly.rs:17:26
   |
17 |     println!("{}", s.n); s.n += 1;
   |                          ^^^^^^^^ cannot assign
```

In all of these cases, the compiler's output can change because our crate or one
of our dependencies broke something, or as a consequence of changes in the Rust
compiler. Both are good reasons to have well conceived compile-fail tests. If we
refactor and mistakenly cause an error that used to be correct to now no longer
be emitted or be emitted in the wrong place, that is important for a test suite
to catch. If the compiler changes something that makes error messages that we
care about substantially worse, it is also important to catch and report as a
compiler issue.

<br>
