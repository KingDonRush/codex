# npm releases

Use the staging helper in the repo root to generate npm tarballs for a release. For
example, to stage the CLI, responses proxy, and SDK packages for version `0.6.0`:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.6.0 \
  --package codex \
  --package codex-responses-api-proxy \
  --package codex-sdk
```

This downloads the native package archive artifacts once, hydrates `vendor/` for each
package, and writes tarballs to `dist/npm/`.

When `--package codex` is provided, the staging helper builds the lightweight
`@openai/codex` meta package plus all platform-native `@openai/codex` variants
that are later published under platform-specific dist-tags.

Use `--package claudex` to stage the separate Claudex meta package. It exposes
only the `claudex` bin and depends on the same platform-native Codex payload
packages as the main CLI, so the installed software has a separate command and
home directory without duplicating native artifacts.

If you need to invoke `build_npm_package.py` directly, run
`codex-cli/scripts/install_native_deps.py --component codex-package` first and pass
`--vendor-src` pointing to the directory that contains the populated `vendor/` tree.
