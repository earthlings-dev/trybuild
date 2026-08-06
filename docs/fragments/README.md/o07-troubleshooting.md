## Troubleshooting

The Rust compiler's diagnostic output can vary as a function of whether the
`rust-src` Rustup component is installed. The compiler will render source
snippets from the standard library if the standard library source is available
locally, and will simply omit snippets if not. This can account for differences
between CI and local development.

If you have compile_fail tests pertaining to standard library traits or types,
you can ensure a consistent environment by adding a rust-toolchain.toml file
with the following content.

```toml
[toolchain]
components = ["rust-src"]
```

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
