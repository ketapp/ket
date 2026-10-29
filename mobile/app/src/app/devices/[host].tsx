'use no memo'; // Reads a live HostConnection — see lib/hosts.ts.

// One paired desktop, as this phone sees it: whether it can be reached right
// now and over what, the ways there are to reach it with the live one marked,
// the phones it trusts, and a way to cut any of them off — this one included,
// which forgets the desktop on both sides. When the desktop can't be reached
// this is the place to fix it: how long it has been down, what each address
// said, and where a new one goes.

import { router, useLocalSearchParams } from 'expo-router';
import type { ReactNode } from 'react';
import { useCallback, useEffect, useState } from 'react';
import { ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import type { Device } from '@ket/remote';

import { hostOf } from '../../components/ConnectionHelp';
import { Close, Desktop, Globe, Link, Padlock, Phone, Plus, Refresh, Tablet, Wifi } from '../../components/icons';
import { Bar, Button, Card, Figure, Heading, IconButton, Prose, Row, Screen, Tag, Value, field } from '../../components/ui';
import { confirm } from '../../lib/confirm';
import { dropConnection, type HostConnection } from '../../lib/connection';
import { useHost } from '../../lib/hosts';
import { forgetHost } from '../../lib/store';
import { font, size, t } from '../../lib/theme';

export default function Devices() {
  const { host: id } = useLocalSearchParams<{ host: string }>();
  const conn = useHost(id);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState('');
  const [adding, setAdding] = useState(false);
  const [address, setAddress] = useState('');
  const [addressError, setAddressError] = useState('');

  const load = useCallback(async (c: HostConnection) => {
    try {
      setDevices(await c.devices());
      setError('');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  // Asked once the desktop is there to ask: a list fetched while it was
  // down would only ever be the error.
  const up = conn?.status === 'connected';
  useEffect(() => {
    if (!conn || !up) return;
    let live = true;
    conn
      .devices()
      .then((list) => {
        if (!live) return;
        setDevices(list);
        setError('');
      })
      .catch((e: unknown) => {
        if (live) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      live = false;
    };
  }, [conn, up]);

  const revoke = async (device: Device) => {
    if (!conn) return;
    const yes = await confirm(`Revoke ${device.name || 'this phone'}?`, 'It will be disconnected and have to pair again.', 'Revoke');
    if (!yes) return;
    await conn.revoke(device.id).then(() => load(conn)).catch((e: Error) => setError(e.message));
  };

  const forget = async () => {
    const yes = await confirm(`Forget ${conn?.host.name ?? 'this desktop'}?`, 'This phone will have to pair again to use it.', 'Forget');
    if (!yes) return;
    const self = devices?.find((d) => d.thisDevice);
    // Tell the desktop first, while there is still a session to say it in; the
    // phone forgets the desktop whether or not that works.
    if (conn && self) await conn.revoke(self.id).catch(() => {});
    dropConnection(id);
    await forgetHost(id);
    router.replace('/settings');
  };

  const addAddress = async () => {
    if (!conn) return;
    try {
      await conn.addAddress(address);
      setAddress('');
      setAddressError('');
      setAdding(false);
    } catch (cause) {
      setAddressError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const removeAddress = async (url: string) => {
    if (!conn) return;
    await conn.removeAddress(url).catch((cause: Error) => setAddressError(cause.message));
  };

  const fromDesktop = conn ? (conn.host.addresses?.length ? conn.host.addresses : [conn.host.relay]) : [];
  const manual = conn?.host.manualAddresses ?? [];
  const failures = conn?.failures ?? {};
  const status = conn?.status ?? 'connecting';

  return (
    <Screen>
      <Bar back title={conn?.host.name ?? 'Desktop'} detail="Settings" />
      <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        {conn ? <Status conn={conn} /> : null}

        <Card style={styles.card}>
          <Heading count={fromDesktop.length + manual.length}>Ways to reach it</Heading>
          {fromDesktop.map((url) => (
            <AddressRow key={`host-${url}`} url={url} conn={conn} failure={status === 'offline' ? failures[url] : undefined} />
          ))}
          {manual.map((url) => (
            <AddressRow key={`manual-${url}`} url={url} conn={conn} manual failure={status === 'offline' ? failures[url] : undefined}>
              <IconButton label={`Remove ${hostOf(url)}`} onPress={() => void removeAddress(url)}>
                <Close size={16} color={t.dim} />
              </IconButton>
            </AddressRow>
          ))}
          {adding ? (
            <View style={styles.well}>
              <Text style={styles.wellLabel}>New address</Text>
              <TextInput
                style={[field, addressError ? styles.fieldBad : null]}
                value={address}
                onChangeText={setAddress}
                placeholder="ws:// or wss:// address"
                placeholderTextColor={t.faint}
                autoCapitalize="none"
                autoCorrect={false}
                autoFocus
                keyboardType="url"
                returnKeyType="done"
                onSubmitEditing={() => void addAddress()}
                accessibilityLabel="New desktop address"
              />
              <Prose color={addressError ? t.failed : t.faint} style={styles.hint}>
                {addressError || 'A work VPN, port forwarding, or your own relay.'}
              </Prose>
              <View style={styles.wellActions}>
                <Button
                  title="Cancel"
                  onPress={() => {
                    setAdding(false);
                    setAddress('');
                    setAddressError('');
                  }}
                />
                <Button kind="primary" title="Add" onPress={() => void addAddress()} />
              </View>
            </View>
          ) : (
            <Row
              small
              mark={<Plus size={16} color={t.dim} />}
              title="Add an address"
              detail="A work VPN, port forwarding, or your own relay"
              onPress={() => setAdding(true)}
            />
          )}
          {!adding && addressError ? (
            <Prose color={t.failed} style={styles.note}>
              {addressError}
            </Prose>
          ) : null}
        </Card>

        <Card style={styles.card}>
          <Heading count={devices ? devices.length : status === 'connected' ? undefined : 'when connected'}>Phones it trusts</Heading>
          {(devices ?? []).map((device) => (
            <Row
              key={device.id}
              small
              mark={isTablet(device.name) ? <Tablet size={16} color={t.dim} /> : <Phone size={16} color={t.dim} />}
              title={device.name || 'Unnamed phone'}
              detail={`${device.connected ? 'Connected now' : 'Not connected'} · paired ${day(Number(device.pairedAt) * 1000)}`}
              meta={device.thisDevice ? <Tag>this phone</Tag> : <Text style={styles.action}>Revoke</Text>}
              onPress={device.thisDevice ? undefined : () => void revoke(device)}
            />
          ))}
          {error ? (
            <Prose color={t.failed} style={styles.note}>
              {error}
            </Prose>
          ) : devices === null ? (
            <Prose style={styles.note}>{status === 'connected' ? 'Asking the desktop…' : 'The desktop lists them once it can be reached.'}</Prose>
          ) : null}
        </Card>

        <View style={styles.forget}>
          <Button kind="danger" title="Forget this desktop" onPress={() => void forget()} style={styles.wide} />
          <Prose color={t.faint} style={styles.forgetNote}>
            {status === 'connected'
              ? 'Removes this phone from the desktop too. Scan a new code to pair again.'
              : 'The desktop can’t be told right now, so it will list this phone until you revoke it there.'}
          </Prose>
        </View>
      </ScrollView>
    </Screen>
  );
}

/** How the desktop stands: reachable or not, over what, since when — and
 * when it is not, what to do about it. */
function Status({ conn }: { conn: HostConnection }) {
  const status = conn.status;
  const color = status === 'connected' ? t.running : status === 'offline' ? t.modified : t.dim;
  const title = status === 'connected' ? 'Connected' : status === 'offline' ? 'Can’t reach it' : 'Connecting…';
  const detail =
    status === 'connected'
      ? `${route(conn.activeAddress, conn)}${conn.upSince ? ` · since ${when(conn.upSince)}` : ''}`
      : status === 'offline'
        ? `${conn.downSince ? `Down since ${when(conn.downSince)} · ` : ''}attempt ${Math.max(conn.attempts, 1)}`
        : 'Trying every saved address';
  return (
    <Card style={styles.status}>
      <View style={styles.statusIcon}>
        <Desktop size={22} color={color} />
      </View>
      <View style={styles.statusBody}>
        <View style={styles.statusText}>
          <Text style={styles.statusTitle}>{title}</Text>
          <Text style={styles.statusDetail} numberOfLines={2}>
            {detail}
          </Text>
        </View>
        {status === 'connected' ? (
          <View style={styles.facts}>
            <Figure label="Protocol" style={styles.fact}>
              <Value>1.{conn.hostMinor}</Value>
            </Figure>
            <Figure label="Paired" style={styles.fact}>
              <Value>{day(conn.host.pairedAt)}</Value>
            </Figure>
            <Figure label="Encrypted" style={styles.fact}>
              <View style={styles.lock}>
                <Padlock size={12} color={t.brand} />
                <Value color={t.brand}>End to end</Value>
              </View>
            </Figure>
          </View>
        ) : status === 'offline' ? (
          <View style={styles.statusActions}>
            <Button kind="primary" title="Try again" icon={<Refresh size={14} color={t.onAccent} />} onPress={() => conn.retry()} />
            <Prose color={t.faint} style={styles.hint}>
              Retrying on its own every few seconds. Is the Mac awake and ket open?
            </Prose>
          </View>
        ) : null}
      </View>
    </Card>
  );
}

/** One address: its kind, where it came from, and — while the desktop can't
 * be reached — what it said when the phone tried it. */
function AddressRow({
  url,
  conn,
  manual,
  failure,
  children,
}: {
  url: string;
  conn: HostConnection | null;
  manual?: boolean;
  failure?: string;
  children?: ReactNode;
}) {
  const live = conn?.status === 'connected' && conn.activeAddress === url;
  const k = kind(url, manual);
  const Icon = k.icon;
  const said = failure ? outcome(failure) : null;
  return (
    <View style={styles.addressRow}>
      <View style={styles.addressText}>
        <Row
          mark={<Icon size={16} color={live ? t.running : t.dim} />}
          title={hostOf(url)}
          detail={manual ? 'Added on this phone' : `${k.name} · from your Mac`}
          meta={
            live ? (
              <Tag color={t.running}>In use</Tag>
            ) : said ? (
              <Text style={[styles.outcome, { color: said.color }]}>{said.word}</Text>
            ) : undefined
          }
        />
      </View>
      {children}
    </View>
  );
}

/** What an address is, from its shape: the Mac's own name on the local
 * network, a Tailscale name, or anything else — a relay. */
function kind(url: string, manual?: boolean): { name: string; icon: typeof Wifi } {
  if (manual) return { name: 'Added on this phone', icon: Link };
  const host = hostOf(url).toLowerCase().replace(/:\d+$/, '');
  if (host.endsWith('.ts.net') || /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host)) return { name: 'Tailscale', icon: Globe };
  if (host.endsWith('.local') || host === 'localhost' || /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host)) {
    return { name: 'Local network', icon: Wifi };
  }
  return { name: 'Relay', icon: Globe };
}

/** Where the open session runs, as a phrase. */
function route(address: string | null, conn: HostConnection): string {
  if (!address) return 'Connected';
  if (conn.host.manualAddresses?.includes(address)) return 'At an address you added';
  switch (kind(address).name) {
    case 'Local network':
      return 'On the local network';
    case 'Tailscale':
      return 'Over Tailscale';
    default:
      return 'Through the relay';
  }
}

/** A dial's failure as a word, coloured by how final it is: no answer may
 * be the wrong network; a refusal is the other end saying no. */
function outcome(message: string): { word: string; color: string } {
  const m = message.toLowerCase();
  if (m.includes('did not answer')) return { word: 'No answer', color: t.modified };
  if (m.includes('cancelled')) return { word: 'Not tried', color: t.faint };
  if (m.includes('refused') || m.includes('declined')) return { word: 'Refused', color: t.failed };
  return { word: 'Failed', color: t.failed };
}

function isTablet(name: string): boolean {
  return /ipad|tablet/i.test(name);
}

/** "6 Oct", with the year once it is not this one. */
function day(ms: number): string {
  const date = new Date(ms);
  const thisYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString([], { day: 'numeric', month: 'short', ...(thisYear ? {} : { year: 'numeric' }) });
}

/** A clock time today, the day otherwise. */
function when(ms: number): string {
  const date = new Date(ms);
  const today = date.toDateString() === new Date().toDateString();
  return today ? date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) : day(ms);
}

const styles = StyleSheet.create({
  scroll: { padding: size.gutter, paddingTop: 4, gap: size.gutter, paddingBottom: 32 },
  card: { padding: 8, gap: 2 },
  note: { padding: 8 },
  status: { flexDirection: 'row', alignItems: 'flex-start', gap: 12, padding: 14 },
  statusIcon: {
    width: 40,
    height: 40,
    borderRadius: size.radiusLg,
    backgroundColor: t.sunken,
    alignItems: 'center',
    justifyContent: 'center',
  },
  statusBody: { flex: 1, minWidth: 0, gap: 12 },
  statusText: { gap: 3, paddingTop: 1 },
  statusTitle: { fontFamily: font.proseSemibold, fontSize: size.title, lineHeight: 18, color: t.text },
  statusDetail: { fontFamily: font.prose, fontSize: size.detail, lineHeight: 17, color: t.dim },
  facts: { flexDirection: 'row', gap: 6 },
  fact: { flex: 1, backgroundColor: t.sunken, borderRadius: size.radius, paddingHorizontal: 10, paddingVertical: 8 },
  lock: { flexDirection: 'row', alignItems: 'center', gap: 5 },
  statusActions: { alignItems: 'flex-start', gap: 8 },
  hint: { fontSize: size.detail, lineHeight: 17 },
  addressRow: { flexDirection: 'row', alignItems: 'center' },
  addressText: { flex: 1, minWidth: 0 },
  outcome: { fontFamily: font.chrome, fontSize: size.caption },
  action: { fontFamily: font.prose, fontSize: size.label, color: t.dim },
  well: { marginTop: 4, padding: 10, borderRadius: size.radiusRow, backgroundColor: t.sunken, gap: 8 },
  wellLabel: { fontFamily: font.proseMedium, fontSize: size.detail, color: t.dim },
  fieldBad: { borderWidth: 1, borderColor: t.failed, backgroundColor: t.card },
  wellActions: { flexDirection: 'row', justifyContent: 'flex-end', gap: 6, paddingTop: 2 },
  forget: { marginTop: 6, paddingHorizontal: 4, gap: 8 },
  wide: { alignSelf: 'stretch', alignItems: 'center' },
  forgetNote: { fontSize: size.detail, lineHeight: 17, textAlign: 'center' },
});
