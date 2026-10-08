import { type ConfigPlugin, withPodfileProperties, withXcodeProject } from "expo/config-plugins.js";

const deploymentTarget = "18.0";

const withIos18: ConfigPlugin = (config) => {
  const withDeploymentProperty = withPodfileProperties(config, (configured) => {
    configured.modResults["ios.deploymentTarget"] = deploymentTarget;
    return configured;
  });

  return withXcodeProject(withDeploymentProperty, (configured) => {
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
};

export default withIos18;
