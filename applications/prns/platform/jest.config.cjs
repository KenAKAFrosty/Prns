module.exports = {
  preset: "jest-expo",
  // Local SDK file dependencies resolve through this consumer's peers.
  modulePaths: ["<rootDir>/../../node_modules"],
  moduleNameMapper: {
    "^@prns-test/host-ffi$": require
      .resolve("personal-rns-expo/package.json")
      .replace(/package\.json$/u, "src/generated/prns_host_uniffi-ffi.ts"),
  },
  testPathIgnorePatterns: ["/node_modules/", "/ios/build/"],
  transformIgnorePatterns: [
    "/node_modules/(?!(.pnpm|effect|personal-rns-expo|react-native|@react-native|@react-native-community|expo|@expo|@expo-google-fonts|react-navigation|@react-navigation|@sentry/react-native|native-base|standard-navigation))",
    "/node_modules/react-native-reanimated/plugin/",
    "/node_modules/@react-native/babel-preset/",
    "/prns-js/dist-cjs/",
  ],
};
