import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  KeyRound,
  MonitorSmartphone,
  PackageCheck,
  RefreshCw,
  ShieldCheck,
  Stamp,
  Trash2,
  UserPlus,
  type LucideIcon,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type {
  DeviceInviteInput,
  DeviceInviteRecord,
  DevicesInvitesSnapshot,
  DeviceVaultStatus,
  Role,
  SignedRoleGrant,
} from "../../types/backend";

function formatDate(value: string | null | undefined) {
  if (!value) return "never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 19);
  return date.toLocaleString(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function securityTone(value: string) {
  if (value === "strong") return "bg-sage/10 text-sage-dark";
  if (value === "dev") return "bg-amber/10 text-amber-dark";
  return "bg-cream-100 text-cream-500";
}

function inviteTone(invite: DeviceInviteRecord) {
  return invite.status === "approved"
    ? "bg-sage/10 text-sage-dark"
    : "bg-cream-100 text-cream-500";
}

export function DevicesView() {
  const [snapshot, setSnapshot] = useState<DevicesInvitesSnapshot | null>(null);
  const [collaboratorName, setCollaboratorName] = useState("");
  const [joinRequest, setJoinRequest] = useState("");
  const [notes, setNotes] = useState("");
  const [inviteRole, setInviteRole] = useState<Role>("collaborator");
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const requestId = useRef(0);

  const loadSnapshot = useCallback(async () => {
    const id = requestId.current + 1;
    requestId.current = id;
    setIsBusy(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<DevicesInvitesSnapshot>(
        "get_devices_invites_snapshot",
      );
      if (requestId.current === id) setSnapshot(result);
    } catch (e) {
      if (requestId.current === id) {
        setError(e instanceof Error ? e.message : "Device snapshot failed.");
      }
    } finally {
      if (requestId.current === id) setIsBusy(false);
    }
  }, []);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  const runSnapshotCommand = async (
    command: string,
    args?: Record<string, unknown>,
  ) => {
    if (isBusy) return;
    setIsBusy(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<DevicesInvitesSnapshot>(
        command,
        args,
      );
      setSnapshot(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Device operation failed.");
    } finally {
      setIsBusy(false);
    }
  };

  const copy = async (id: string, value: string | null | undefined) => {
    if (!value) return;
    await navigator.clipboard.writeText(value);
    setCopied(id);
    window.setTimeout(() => setCopied(null), 1200);
  };

  const approveInvite = async () => {
    if (!collaboratorName.trim() || !joinRequest.trim()) return;
    const input: DeviceInviteInput = {
      collaboratorName: collaboratorName.trim(),
      joinRequest: joinRequest.trim(),
      notes: notes.trim() || null,
      role: inviteRole,
    };
    await runSnapshotCommand("approve_device_invite", { input });
    setCollaboratorName("");
    setJoinRequest("");
    setNotes("");
    setInviteRole("collaborator");
  };

  // Issue the admin-signed role grant for an approved device and copy it to the
  // clipboard so the admin can send it back. The collaborator pastes it in their
  // onboarding; their app verifies it against the bundled trust anchor.
  const issueGrant = async (invite: DeviceInviteRecord) => {
    if (!invite.signingPublicKey) {
      setError(
        "This device has no signing key. Ask the collaborator to resend a full join request (JSON), not a raw public key.",
      );
      return;
    }
    if (isBusy) return;
    setIsBusy(true);
    setError(null);
    try {
      const grant = await invokeBackendCommand<SignedRoleGrant>(
        "issue_role_grant",
        {
          role: invite.role ?? "collaborator",
          subjectPublicKey: invite.publicKey,
          subjectSigningPublicKey: invite.signingPublicKey,
          subjectFingerprint: invite.publicKeyFingerprint,
          expiresInDays: 365,
        },
      );
      await navigator.clipboard.writeText(JSON.stringify(grant, null, 2));
      setCopied(`grant:${invite.id}`);
      window.setTimeout(() => setCopied(null), 1500);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Issuing the role grant failed.");
    } finally {
      setIsBusy(false);
    }
  };

  // One-click admin setup: bake THIS device's signing public key into config.json
  // as the trust anchor. Only the public half is written; the private key stays in
  // the vault. Run in the dev build, then package + distribute.
  const bakeTrustAnchor = async () => {
    if (isBusy) return;
    setIsBusy(true);
    setError(null);
    try {
      const key = await invokeBackendCommand<string>("bake_trust_anchor");
      setError(null);
      setCopied(`anchor-baked:${key.slice(0, 12)}`);
      window.setTimeout(() => setCopied(null), 4000);
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "Could not set the trust anchor.",
      );
    } finally {
      setIsBusy(false);
    }
  };

  const localDevice = snapshot?.localDevice;
  const activeInvites = useMemo(
    () =>
      snapshot?.invites.filter((invite) => invite.status === "approved") ?? [],
    [snapshot?.invites],
  );

  return (
    <div className="max-w-7xl space-y-5">
      <section className="rounded-lg border border-cream-200 bg-white p-4">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div className="min-w-0">
            <div className="mb-3 flex items-center gap-3">
              <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-teal/10">
                <MonitorSmartphone className="h-5 w-5 text-teal" />
              </div>
              <div>
                <h2 className="text-sm font-semibold text-cream-800">
                  Devices & Invites
                </h2>
                <p className="text-[12px] text-cream-400">
                  Approve devices before encrypted workspace bootstrap.
                </p>
              </div>
            </div>
            <p className="max-w-3xl text-[12px] leading-5 text-cream-500">
              Collaborators generate a device join request from their app. You
              approve that public key here. Future workspace packages can then
              be encrypted for approved devices only.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void loadSnapshot()}
              disabled={isBusy}
              data-help-title="This reloads local device and invite state."
              data-help-lines="Reload reads this app's device metadata and approved invite list.|It does not create a key, upload files, or contact a cloud provider.|Use it after another local tool changes device state.|Private keys are never returned to the UI."
              className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
            >
              <RefreshCw
                className={`h-3.5 w-3.5 ${isBusy ? "animate-spin" : ""}`}
              />
              Reload
            </button>
            <button
              type="button"
              onClick={() =>
                void runSnapshotCommand("ensure_local_device_identity")
              }
              disabled={isBusy || localDevice?.configured}
              data-help-title="This creates this app installation's device identity."
              data-help-lines="The app generates an X25519 keypair locally.|The private key is saved in the OS credential vault: Keychain on macOS or Credential Manager on Windows.|The public key becomes the join request you can share.|Do this once per device installation."
              className="inline-flex items-center gap-2 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              <KeyRound className="h-3.5 w-3.5" />
              Create device
            </button>
          </div>
        </div>
      </section>

      {error && (
        <div className="rounded-lg border border-coral/20 bg-coral/[0.04] px-4 py-3 text-[12px] font-medium text-coral-dark">
          {error}
        </div>
      )}

      <section className="grid gap-4 lg:grid-cols-[minmax(0,0.95fr)_minmax(0,1.05fr)]">
        <LocalDeviceCard
          device={localDevice}
          copied={copied}
          isBusy={isBusy}
          onCopy={(id, value) => void copy(id, value)}
          onBakeAnchor={() => void bakeTrustAnchor()}
          onReset={() => void runSnapshotCommand("reset_local_device_identity")}
        />

        <section
          className="rounded-lg border border-cream-200 bg-white p-4"
          data-help-title="Approve a collaborator's device join request."
          data-help-lines="A join request contains only public device information and an X25519 public key.|Approving it does not give the collaborator files yet.|It means future encrypted bootstrap packages may include a package key encrypted for that device.|If the collaborator reinstalls the app or changes laptop, approve a new request."
        >
          <div className="mb-3 flex items-center gap-2">
            <UserPlus className="h-4 w-4 text-terracotta" />
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Approve Invite
            </h3>
          </div>
          <div className="grid gap-2">
            <input
              value={collaboratorName}
              onChange={(event) => setCollaboratorName(event.target.value)}
              placeholder="Collaborator name"
              data-help-title="This names the person who owns the device."
              data-help-lines="Use a human-readable name, not a token or password.|The name helps you revoke the right device later.|It is stored locally in the app invite list.|It is not used as cryptographic identity."
              className="rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <textarea
              value={joinRequest}
              onChange={(event) => setJoinRequest(event.target.value)}
              placeholder="Paste device join request JSON or raw public key"
              rows={5}
              data-help-title="Paste the collaborator's device join request here."
              data-help-lines="The safest format is the JSON copied from their Devboule app.|A raw 32-byte X25519 public key in hex is also accepted for emergency/manual setup.|Never paste a private key here.|If this request came through chat or email, compare the fingerprint with the collaborator out-of-band."
              className="resize-none rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <input
              value={notes}
              onChange={(event) => setNotes(event.target.value)}
              placeholder="Notes, e.g. Ada MacBook Pro"
              className="rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <label className="flex items-center justify-between gap-3 rounded-lg border border-cream-200 bg-cream-50 px-3 py-2">
              <span className="text-[12px] font-medium text-cream-600">
                Role
              </span>
              <select
                value={inviteRole}
                onChange={(event) => setInviteRole(event.target.value as Role)}
                data-help-title="The access role this device gets after onboarding."
                data-help-lines="Collaborator: work surfaces only (projects, agents, workspace, oracle), no admin pages or credential writes.|Admin: full access — grant sparingly.|The role is bound to the device's signing key in a grant you sign and send back.|Real cloud limits come from the scoped token you give them, not this setting alone."
                className="rounded-md border border-cream-200 bg-white px-2 py-1 text-[12px] font-semibold text-cream-700 outline-none focus:border-terracotta-200"
              >
                <option value="collaborator">Collaborator</option>
                <option value="admin">Admin</option>
              </select>
            </label>
            <button
              type="button"
              onClick={() => void approveInvite()}
              disabled={
                isBusy || !collaboratorName.trim() || !joinRequest.trim()
              }
              data-help-title="This approves the pasted device for future packages."
              data-help-lines="Approve stores the public key and fingerprint locally.|It does not upload a package or share source code yet.|Future package encryption should target approved fingerprints only.|Revoke the invite if the laptop is lost or the collaborator leaves."
              className="inline-flex items-center justify-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              <ShieldCheck className="h-3.5 w-3.5" />
              Approve device
            </button>
          </div>
        </section>
      </section>

      <section className="grid gap-3 md:grid-cols-4">
        <FlowCard
          icon={MonitorSmartphone}
          title="1. Collaborator"
          text="Installs the app on Mac and creates a device identity."
        />
        <FlowCard
          icon={Copy}
          title="2. Join request"
          text="Sends you the public join request, not a password."
        />
        <FlowCard
          icon={ShieldCheck}
          title="3. Approval"
          text="You approve the fingerprint in this page."
        />
        <FlowCard
          icon={PackageCheck}
          title="4. Bootstrap"
          text="Workspace packages can be encrypted for approved devices."
        />
      </section>

      <section className="rounded-lg border border-cream-200 bg-white p-4">
        <div className="mb-3 flex items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-teal" />
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Approved Devices
            </h3>
          </div>
          <span className="rounded-md bg-cream-100 px-2 py-1 text-[10px] font-semibold text-cream-500">
            {activeInvites.length} active
          </span>
        </div>
        {!snapshot?.invites.length ? (
          <div className="rounded-lg border border-dashed border-cream-200 bg-cream-50 p-5 text-center">
            <AlertTriangle className="mx-auto mb-2 h-5 w-5 text-cream-300" />
            <p className="text-[13px] font-semibold text-cream-700">
              No collaborator devices approved yet.
            </p>
            <p className="mt-1 text-[12px] text-cream-400">
              Paste a join request above when a collaborator is ready.
            </p>
          </div>
        ) : (
          <div className="grid gap-3 lg:grid-cols-2">
            {snapshot.invites.map((invite) => (
              <InviteCard
                key={invite.id}
                invite={invite}
                copied={copied}
                isBusy={isBusy}
                onCopy={(id, value) => void copy(id, value)}
                onIssueGrant={() => void issueGrant(invite)}
                onRevoke={() =>
                  void runSnapshotCommand("revoke_device_invite", {
                    inviteId: invite.id,
                  })
                }
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function LocalDeviceCard({
  device,
  copied,
  isBusy,
  onCopy,
  onBakeAnchor,
  onReset,
}: {
  device: DeviceVaultStatus | null | undefined;
  copied: string | null;
  isBusy: boolean;
  onCopy: (id: string, value: string | null | undefined) => void;
  onBakeAnchor: () => void;
  onReset: () => void;
}) {
  const anchorBaked = copied?.startsWith("anchor-baked:") ?? false;
  return (
    <section
      className="rounded-lg border border-cream-200 bg-white p-4"
      data-help-title="This device identity is how encrypted packages target this app install."
      data-help-lines="The public key can be shared with an admin or another Devboule install.|The private key stays inside the operating-system credential vault.|On macOS that means Keychain and Touch ID/macOS auth; on Windows it means Credential Manager and Windows Hello.|If you reset this identity, old packages encrypted to the previous key cannot be opened by this install."
    >
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="mb-2 flex items-center gap-2">
            <KeyRound className="h-4 w-4 text-teal" />
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              This Device
            </h3>
          </div>
          <p className="truncate text-[13px] font-semibold text-cream-800">
            {device?.deviceName ?? "No identity yet"}
          </p>
          <p className="mt-1 truncate font-mono text-[10px] text-cream-400">
            {device?.deviceId ??
              "Create this device before sharing a join request"}
          </p>
        </div>
        <span
          className={`rounded-md px-2 py-1 text-[10px] font-semibold ${securityTone(device?.securityLevel ?? "unknown")}`}
        >
          {device?.securityLevel ?? "not ready"}
        </span>
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <DeviceFact label="Platform" value={device?.platform ?? "unknown"} />
        <DeviceFact label="Vault" value={device?.vaultBackend ?? "OS vault"} />
        <DeviceFact
          label="Unlock"
          value={device?.biometricLabel ?? "OS unlock"}
        />
        <DeviceFact
          label="Fingerprint"
          value={device?.publicKeyFingerprint ?? "not created"}
        />
      </div>

      {device?.message && (
        <p className="mt-3 rounded-lg bg-cream-50 px-3 py-2 text-[11px] leading-4 text-cream-500">
          {device.message}
        </p>
      )}

      <div className="mt-3 flex flex-wrap gap-2">
        <button
          type="button"
          onClick={() => onCopy("join-request", device?.joinRequest)}
          disabled={!device?.joinRequest}
          data-help-title="This copies this device's join request."
          data-help-lines="Send this to the admin who will approve your device.|It contains only public key material and device metadata.|It is safe to send over normal chat, but verify the fingerprint before approving high-trust access.|The private key is not included."
          className="inline-flex items-center gap-2 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
        >
          <Copy className="h-3.5 w-3.5" />
          {copied === "join-request" ? "Copied" : "Copy join request"}
        </button>
        <button
          type="button"
          onClick={() => onCopy("trust-anchor", device?.signingPublicKey)}
          disabled={!device?.signingPublicKey}
          data-help-title="Copy this device's signing key to use as the trust anchor."
          data-help-lines="Only relevant for the admin install: paste this into config.json trustAnchor.signingPublicKey before distributing the app.|Collaborators' apps verify your role grants against this key.|While the anchor is empty, every grant fails closed and a fresh build treats itself as admin.|This is a public key — safe to copy."
          className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
        >
          <ShieldCheck className="h-3.5 w-3.5" />
          {copied === "trust-anchor" ? "Copied" : "Copy trust anchor"}
        </button>
        <button
          type="button"
          onClick={onBakeAnchor}
          disabled={isBusy || !device?.signingPublicKey}
          data-help-title="Write this device's public key into config.json as the trust anchor."
          data-help-lines="One click instead of copy-paste: bakes YOUR signing public key into config.json.|Only the public half is written — your private key stays in the OS vault.|Run this in the dev build, then package and distribute that build to collaborators.|In a packaged (read-only) build it will report that it cannot write — that is expected."
          className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
        >
          <ShieldCheck className="h-3.5 w-3.5" />
          {anchorBaked ? "Anchor set ✓" : "Set this device as admin"}
        </button>
        <button
          type="button"
          onClick={onReset}
          disabled={isBusy}
          data-help-title="This regenerates this app install's device keypair."
          data-help-lines="Reset only when this device is compromised, reinstalled, or incorrectly created.|Old packages encrypted to the previous public key will not unlock here anymore.|Tell admins to approve the new fingerprint and revoke the old one.|This does not affect GitHub, Cloudflare, Scaleway, or project files."
          className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-coral-dark disabled:opacity-60"
        >
          <RefreshCw className="h-3.5 w-3.5" />
          Reset key
        </button>
      </div>
    </section>
  );
}

function InviteCard({
  invite,
  copied,
  isBusy,
  onCopy,
  onIssueGrant,
  onRevoke,
}: {
  invite: DeviceInviteRecord;
  copied: string | null;
  isBusy: boolean;
  onCopy: (id: string, value: string | null | undefined) => void;
  onIssueGrant: () => void;
  onRevoke: () => void;
}) {
  return (
    <article className="rounded-lg border border-cream-200 bg-cream-50 p-3">
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate text-[13px] font-semibold text-cream-800">
            {invite.collaboratorName}
          </p>
          <p className="truncate text-[11px] text-cream-500">
            {invite.deviceName} / {invite.platform}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <span className="rounded-md bg-terracotta-50 px-2 py-1 text-[10px] font-semibold text-terracotta-600">
            {invite.role ?? "collaborator"}
          </span>
          <span
            className={`rounded-md px-2 py-1 text-[10px] font-semibold ${inviteTone(invite)}`}
          >
            {invite.status}
          </span>
        </div>
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        <DeviceFact label="Fingerprint" value={invite.publicKeyFingerprint} />
        <DeviceFact label="Approved" value={formatDate(invite.approvedAt)} />
      </div>
      {invite.notes && (
        <p className="mt-2 rounded-md bg-white px-2 py-1 text-[11px] text-cream-500">
          {invite.notes}
        </p>
      )}
      <div className="mt-3 flex flex-wrap gap-2">
        <button
          type="button"
          onClick={() => onCopy(`pub:${invite.id}`, invite.publicKey)}
          className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-terracotta"
        >
          <Copy className="h-3 w-3" />
          {copied === `pub:${invite.id}` ? "Copied" : "Public key"}
        </button>
        {invite.status === "approved" && (
          <button
            type="button"
            onClick={onIssueGrant}
            disabled={isBusy || !invite.signingPublicKey}
            title={
              invite.signingPublicKey
                ? undefined
                : "No signing key in this invite — ask for a full join request."
            }
            data-help-title="Sign a role grant for this device and copy it."
            data-help-lines="The grant binds this device's signing key to its role and is signed with YOUR admin key.|Send it back to the collaborator; their app verifies it against the bundled trust anchor and opens in that role.|It expires in 365 days; re-issue to renew.|Real cloud limits come from the scoped token you give them, not this grant."
            className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-teal disabled:opacity-60"
          >
            <Stamp className="h-3 w-3" />
            {copied === `grant:${invite.id}` ? "Copied" : "Issue grant"}
          </button>
        )}
        {invite.status === "approved" && (
          <button
            type="button"
            onClick={onRevoke}
            disabled={isBusy}
            data-help-title="This revokes the approved device."
            data-help-lines="Revoked devices should not receive new encrypted workspace package keys.|This does not delete packages already downloaded or files already decrypted on that collaborator's machine.|Use it when a laptop is lost, replaced, or a collaborator leaves.|For strong offboarding, also rotate cloud and GitHub permissions."
            className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-2 py-1 text-[10px] font-semibold text-cream-500 hover:text-coral-dark disabled:opacity-60"
          >
            <Trash2 className="h-3 w-3" />
            Revoke
          </button>
        )}
      </div>
    </article>
  );
}

function DeviceFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg bg-white px-3 py-2">
      <p className="text-[9px] font-semibold uppercase tracking-widest text-cream-400">
        {label}
      </p>
      <p
        className="mt-1 truncate font-mono text-[11px] text-cream-700"
        title={value}
      >
        {value}
      </p>
    </div>
  );
}

function FlowCard({
  icon: Icon,
  title,
  text,
}: {
  icon: LucideIcon;
  title: string;
  text: string;
}) {
  return (
    <div className="rounded-lg border border-cream-200 bg-white p-4">
      <div className="mb-3 flex items-center justify-between">
        <Icon className="h-4 w-4 text-terracotta" />
        <CheckCircle2 className="h-3.5 w-3.5 text-cream-300" />
      </div>
      <p className="text-[13px] font-semibold text-cream-800">{title}</p>
      <p className="mt-1 text-[11px] leading-4 text-cream-500">{text}</p>
    </div>
  );
}
