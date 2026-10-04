# Bundled license texts

Some packages that `scripts/bundle-cli.mjs` bundles into `misao-<version>.mjs` publish no license
file. For those, the license text is kept here as `<scope>-<name>.LICENSE` (`@xterm/headless` ->
`xterm-headless.LICENSE`) and is used for `misao-<version>.LICENSES.txt` instead. The files are
verbatim copies of the upstream text; do not edit them.

| File | Package | Source |
|---|---|---|
| `xterm-headless.LICENSE` | `@xterm/headless` (MIT) | https://github.com/xtermjs/xterm.js/blob/c58ea3637f3968e0e6e79cd92cf9aace7ef89ee2/LICENSE (commit `c58ea3637f3968e0e6e79cd92cf9aace7ef89ee2`) |

Why: the published `@xterm/headless` tarball has no LICENSE file, but MIT requires the copyright
and permission notice to accompany copies of the software.
