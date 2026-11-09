// The EAS project this app builds under stays out of the repository: it is
// read from KET_EAS_PROJECT_ID and KET_EAS_OWNER, set in .env.local (ignored) on a
// developer's machine and as EAS environment variables on the build server.
// Without them the app config is app.json as it stands.
const path = require('path');

module.exports = ({ config, projectRoot }) => {
  // Expo CLI loads .env.local on its own; EAS CLI does not.
  if (!process.env.KET_EAS_PROJECT_ID) {
    try {
      process.loadEnvFile(path.join(projectRoot, '.env.local'));
    } catch {
      // No .env.local, as on the build server and in a fresh clone.
    }
  }
  const projectId = process.env.KET_EAS_PROJECT_ID;
  const owner = process.env.KET_EAS_OWNER;
  return {
    ...config,
    ...(owner && { owner }),
    ...(projectId && { extra: { ...config.extra, eas: { ...config.extra?.eas, projectId } } }),
  };
};
