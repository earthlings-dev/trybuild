## Workflow

There are two ways to update the _*.stderr_ files as you iterate on your test
cases or your library; handwriting them is not recommended.

First, if a test case is being run as compile\_fail but a corresponding
_*.stderr_ file does not exist, the test runner will save the actual compiler
output with the right filename into a directory called *wip* within the
directory containing Cargo.toml. So you can update these files by deleting them,
running `cargo test`, and moving all the files from *wip* into your testcase
directory.

<p align="center">
<a href="#workflow">
<img src="https://user-images.githubusercontent.com/1940490/57186579-7cd51580-6e96-11e9-9f19-54dcecc9fbba.png" width="600">
</a>
</p>

Alternatively, run `cargo test` with the environment variable
`TRYBUILD=overwrite` to skip the *wip* directory and write all compiler output
directly in place. You'll want to check `git diff` afterward to be sure the
compiler's output is what you had in mind.

<br>
