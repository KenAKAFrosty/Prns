module.exports = {
  preset: "jest-expo",
  setupFilesAfterEnv: ["<rootDir>/src/testkit/setup.ts"],
  testPathIgnorePatterns: ["/node_modules/", "/dist/"],
};
