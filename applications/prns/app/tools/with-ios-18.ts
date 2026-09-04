import { type ConfigPlugin, withXcodeProject } from "@expo/config-plugins";

const deploymentTarget = "18.0";

const withIos18: ConfigPlugin = (config) =>
  withXcodeProject(config, (configured) => {
    const configurations = configured.modResults.pbxXCBuildConfigurationSection();
    for (const [key, entry] of Object.entries(configurations)) {
      if (key.endsWith("_comment") || entry === undefined || typeof entry !== "object") {
        continue;
      }
      const buildSettings = (entry as { buildSettings?: Record<string, unknown> }).buildSettings;
      if (buildSettings !== undefined) {
        buildSettings.IPHONEOS_DEPLOYMENT_TARGET = deploymentTarget;
      }
    }
    return configured;
  });

export default withIos18;
