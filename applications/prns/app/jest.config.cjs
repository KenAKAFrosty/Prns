module.exports = {
  preset: "jest-expo",
  moduleNameMapper: {
    "^@/native/runtime-provider$": "<rootDir>/src/native/runtime-provider.web.ts",
  },
  setupFilesAfterEnv: ["<rootDir>/src/testkit/setup.ts"],
  testPathIgnorePatterns: ["/node_modules/", "/dist/"],
  transformIgnorePatterns: [
    "/node_modules/(?!(.pnpm|effect|react-native|@react-native|@react-native-community|expo|@expo|@expo-google-fonts|react-navigation|@react-navigation|@sentry/react-native|native-base|standard-navigation))",
    "/node_modules/react-native-reanimated/plugin/",
    "/node_modules/@react-native/babel-preset/",
    "/prns-js/dist-cjs/",
  ],
};
