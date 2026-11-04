const path = require('node:path');
const { realpathSync } = require('node:fs');
const { getDefaultConfig } = require('expo/metro-config');

const config = getDefaultConfig(__dirname);
// This file-linked package lives outside the applications workspace. Metro can
// resolve it through node_modules without watching its rebuilt dist files.
// Watch its real package root, preserving Expo's workspace and resolver defaults.
const personalRns = path.dirname(realpathSync(require.resolve('personal-rns/package.json')));
require(path.join(personalRns, 'scripts/check-built-contracts.cjs')).checkBuiltContracts(personalRns);
config.watchFolders = [...new Set([...config.watchFolders, personalRns])];

module.exports = config;
