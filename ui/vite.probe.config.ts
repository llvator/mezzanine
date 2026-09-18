/**
 * Throwaway dev-server config for a live UI check.
 *
 * The repo's own config hardcodes `localhost:3000`, where a `mezz watch` for
 * a different repo usually already is. This spreads it and moves only the
 * proxy target, so the probe drives *this* checkout against a backend of its
 * own choosing (`PROBE_API_PORT`).
 */
import base from './vite.config';

const port = process.env.PROBE_API_PORT ?? '3971';
const target = `http://localhost:${port}`;

export default {
  ...base,
  server: {
    ...(base as any).server,
    proxy: {
      '/events': { target, changeOrigin: true },
      '/api': { target, changeOrigin: true },
    },
  },
};
