This is an Expo/React Native mobile application. Prioritize mobile-first patterns, performance, and cross-platform compatibility.

## Expo has changed — do not trust your training data

Expo ships breaking changes every SDK release. APIs you remember are likely renamed, moved, or removed. Before writing any code that touches an Expo, EAS, or React Native API:

1. Read the major version of the `expo` package in `package.json`.
2. Fetch the matching versioned docs: `https://docs.expo.dev/versions/v<major>.0.0/`
3. For anything else, fetch https://docs.expo.dev/llms.txt — an index of all Expo docs with corrections to common LLM misconceptions. Follow its links to the specific page you need; never answer from memory.

## Commands

Use `bunx` instead of `npx` if the project uses bun (`bun.lock` present).

```bash
npx expo install <package>  # ALWAYS use instead of npm/yarn/pnpm/bun add — resolves SDK-compatible versions
npx expo start              # start the dev server
npx expo lint               # lint
npx tsc --noEmit            # typecheck
npx expo-doctor             # diagnose dependency and config issues
npx expo install --fix      # fix incompatible package versions
```

When committing app changes, let the repository's pre-commit hook lint the
staged files and typecheck the app. Run `npx expo lint` and `npx tsc --noEmit`
yourself before wrapping up if you are working without a commit or the hooks
are off.

## Navigation & Routing

- Use **Expo Router** for all navigation. Routes live in `src/app/` — every file there is a screen, `_layout.tsx` files define navigators. Keep non-route code (components, hooks, utils) outside `src/app/`.
- Import `Link`, `router`, and `useLocalSearchParams` from `expo-router`.
- Docs: https://docs.expo.dev/router/introduction.md

## Building — locally, never EAS

This project does not use EAS. Build on the Mac:

- Development: `npx expo run:ios` (simulator, or a USB-connected iPhone signed with
  a free Apple ID) and `npx expo run:android`.
- Release: `npx expo prebuild`, then archive in Xcode / Gradle. No `eas build`,
  `eas submit` or `eas update`.

## Rules

- Build every screen from the shared components: `src/components/ui/` (listed in its `index.ts`), `Sheet.tsx` and `FloatMenu.tsx`. Never draw a one-off button, row, field or menu. If nothing fits, say what's missing and ask. The repository's root `AGENTS.md` has the full rule, and its 2,000-line file limit applies here too.
- If `ios/` and `android/` directories do not exist, they are generated (Continuous Native Generation). Never create or edit them by hand — configure native behavior in `app.json` and config plugins.
- Expo Go only includes its bundled native modules. After adding a library with native code, the app needs a local development build: `npx expo run:ios|android`.
- Prefer recommended Expo modules over third-party libraries, and check your available skills before adding dependencies. Docs: https://docs.expo.dev/versions/latest/index.md
