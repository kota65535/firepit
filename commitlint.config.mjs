// Conventional Commits enforcement for commitlint.
//
// @commitlint/cli depends on @commitlint/config-conventional, so the preset is installed alongside
// it and resolves without a node_modules / package.json in this Rust repo.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    "scope-case": [2, "always", "lower-case"],
  },
};
