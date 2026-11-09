// What a desktop's address is, from its shape, and whether this phone knows
// one that works away from the desktop's own network. Kept apart from
// `addresses.ts` so screens can ask without loading the protocol package.

import type { SavedHost } from './store';

/** ket's own relay, which a Mac links to when Reach it from anywhere is on
 * in its Settings › Devices. */
export const KET_RELAY = 'relay.ketapp.dev';

/** The Mac's own name or address on its local network; its tailnet address
 * or Tailscale name; ket's relay; or any other relay. */
export type AddressKind = 'local' | 'tailnet' | 'ket-relay' | 'relay';

export function addressKind(url: string): AddressKind {
  let host: string;
  try {
    host = new URL(url).host;
  } catch {
    host = url;
  }
  host = host.toLowerCase().replace(/:\d+$/, '');
  if (host === KET_RELAY) return 'ket-relay';
  if (host.endsWith('.ts.net') || /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host)) return 'tailnet';
  // A bare name with no dot only resolves on a local network.
  const bare = !host.includes('.') && !host.startsWith('[');
  if (bare || host.endsWith('.local') || /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host)) {
    return 'local';
  }
  return 'relay';
}

/** Whether this phone knows a way to reach `host` from off its network: the
 * Mac's tailnet address, a relay, or an address its owner added. */
export function reachesAway(host: SavedHost): boolean {
  if (host.manualAddresses?.length) return true;
  return [...(host.addresses ?? []), host.relay].some((url) => addressKind(url) !== 'local');
}
