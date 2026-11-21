// https://docs.expo.dev/guides/using-eslint/
const { defineConfig } = require('eslint/config');
const expoConfig = require("eslint-config-expo/flat");

module.exports = defineConfig([
  expoConfig,
  {
    ignores: ["dist/*"],
  },
  // The React Compiler caches every component it can read, and skips one it
  // can't without a word: a BigInt literal, a try with a finally, a
  // conditional inside a try. These make that an error, and ban opting out.
  {
    rules: {
      "react-hooks/todo": "error",
      "react-hooks/invariant": "error",
      "react-hooks/unsupported-syntax": "error",
      "no-restricted-syntax": [
        "error",
        {
          selector: "ExpressionStatement[directive=/^use no (memo|forget)$/]",
          message: "Don't opt out of the React Compiler: change what it can't read instead.",
        },
      ],
    },
  },
]);
