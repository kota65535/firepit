// Conventional Commits enforcement for commitlint.
//
// The rules below mirror @commitlint/config-conventional (v21.1.0):
// https://github.com/conventional-changelog/commitlint/blob/v21.1.0/@commitlint/config-conventional/src/index.ts
//
// They are inlined rather than pulled in via `extends` because commitlint and
// the preset are installed under separate mise (npm backend) prefixes, so
// commitlint cannot resolve the preset package at runtime. Keeping the rules
// here avoids a node_modules / package.json in this Rust repo.
export default {
  rules: {
    "body-leading-blank": [1, "always"],
    "body-max-line-length": [2, "always", 100],
    "footer-leading-blank": [1, "always"],
    "footer-max-line-length": [2, "always", 100],
    "header-max-length": [2, "always", 100],
    "header-trim": [2, "always"],
    // Not part of @commitlint/config-conventional; placed in alphabetical order.
    "scope-case": [2, "always", "lower-case"],
    "subject-case": [
      2,
      "never",
      ["sentence-case", "start-case", "pascal-case", "upper-case"],
    ],
    "subject-empty": [2, "never"],
    "subject-full-stop": [2, "never", "."],
    "type-case": [2, "always", "lower-case"],
    "type-empty": [2, "never"],
    "type-enum": [
      2,
      "always",
      [
        "build",
        "chore",
        "ci",
        "docs",
        "feat",
        "fix",
        "perf",
        "refactor",
        "revert",
        "style",
        "test",
      ],
    ],
  },
};
