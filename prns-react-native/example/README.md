# Expo host example

This app opens a persistent PRNS host, displays its state, connects to an editable
TCP peer and stops the host. Follow the SDK's [source quickstart](../README.md#source-quickstart)
to build the core JavaScript contract and selected native image first.

From this directory:

```sh
npm ci
npm run android -- --device
# Or, for an iOS simulator:
npm run ios
```

The app's `postinstall` applies the SDK-shipped patch to the exact published
`@ubjs/react-native` `0.31.0-6` package using dev-only `patch-package`. If lifecycle
scripts are disabled, run `npm run patch:runtime` after installation and before a
native build. Repeating that command is safe. The [runtime patch requirements](../README.md#required-runtime-patch)
also apply to other apps consuming the SDK.

Expo Go cannot load the native image. Physical iOS installation requires the
consumer's signing configuration. The Expo 58 template supplies iOS scene startup;
retain its `SceneDelegate` integration when customizing the native project.
