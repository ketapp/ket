// The protocol package lives beside the app (mobile/remote), linked with a
// file: dependency; Metro has to be told to watch it.
const path = require('node:path');
const { getDefaultConfig } = require('expo/metro-config');

const config = getDefaultConfig(__dirname);
config.watchFolders = [path.resolve(__dirname, '../remote')];
module.exports = config;
