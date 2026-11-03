const { getDefaultConfig } = require('expo/metro-config');
const path = require('node:path');
const personalRns = path.resolve(__dirname, '../../prns-js');
require(path.join(personalRns, 'scripts/check-built-contracts.cjs')).checkBuiltContracts(personalRns);
const config = getDefaultConfig(__dirname);
config.watchFolders = [path.resolve(__dirname, '..'), path.resolve(__dirname, '../../prns-js')];
config.resolver.nodeModulesPaths = [path.resolve(__dirname, 'node_modules')];
module.exports = config;
