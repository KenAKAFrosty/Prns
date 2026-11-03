const { withGradleProperties } = require("expo/config-plugins");

// Platform packaging only. Generated SDK namespaces and the selected native
// image are linked once by PrnsHostExpo / personal-rns-expo.
module.exports = function withPrnsAppPlatform(config) {
  config.ios = { ...config.ios, deploymentTarget: "18.0" };
  config.android = { ...config.android, allowBackup: false };
  return withGradleProperties(config, configured => {
    configured.modResults = configured.modResults.filter(entry =>
      entry.type !== "property" || entry.key !== "android.minSdkVersion");
    configured.modResults.push({ type: "property", key: "android.minSdkVersion", value: "29" });
    return configured;
  });
};
