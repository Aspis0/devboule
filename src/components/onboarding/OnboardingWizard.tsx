import { Copy, Fingerprint, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { invokeBackendCommand, useAppActions } from "../../context/AppContext";
import type {
  DevicesInvitesSnapshot,
  LocalRoleStatus,
} from "../../types/backend";

type Step = "identity" | "grant" | "done";

/**
 * First-run onboarding for a fresh COLLABORATOR install (no role grant yet, and
 * not the admin). Three steps: create the device identity + copy the join
 * request, paste the admin's signed role grant, then enter the app in that role.
 * The admin install never reaches this (it is provisioned by the trust anchor).
 *
 * `onSkip` is the anti-lockout escape (audit H1): the collaborator can always
 * continue into the app with the default least-privilege role even if their grant
 * never verifies, so they can never get wedged here.
 */
export function OnboardingWizard({ onSkip }: { onSkip: () => void }) {
  const { refreshRole } = useAppActions();
  const [step, setStep] = useState<Step>("identity");
  const [snapshot, setSnapshot] = useState<DevicesInvitesSnapshot | null>(null);
  const [grant, setGrant] = useState("");
  const [adopted, setAdopted] = useState<LocalRoleStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  const loadSnapshot = useCallback(async () => {
    try {
      const result = await invokeBackendCommand<DevicesInvitesSnapshot>(
        "get_devices_invites_snapshot",
      );
      setSnapshot(result);
    } catch {
      // The identity may simply not exist yet; the button below creates it.
    }
  }, []);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  const joinRequest = snapshot?.localDevice.joinRequest ?? null;

  const createIdentity = async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await invokeBackendCommand<DevicesInvitesSnapshot>(
        "ensure_local_device_identity",
      );
      setSnapshot(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not create the device identity.");
    } finally {
      setBusy(false);
    }
  };

  const copyJoinRequest = async () => {
    if (!joinRequest) return;
    await navigator.clipboard.writeText(joinRequest);
    setCopied(true);
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
  };

  const adoptGrant = async () => {
    const raw = grant.trim();
    if (!raw) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      setError("That is not valid grant JSON. Paste exactly what the admin sent.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const status = await invokeBackendCommand<LocalRoleStatus>(
        "verify_and_adopt_role_grant",
        { grant: parsed },
      );
      setAdopted(status);
      setStep("done");
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "The role grant could not be verified.",
      );
    } finally {
      setBusy(false);
    }
  };

  const enterApp = async () => {
    await refreshRole();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-100 p-6">
      <div className="w-full max-w-lg rounded-2xl border border-cream-200 bg-white p-6 shadow-soft-md">
        <div className="mb-5 flex items-center gap-2">
          <div className="flex h-9 w-9 items-center justify-center rounded-2xl bg-terracotta">
            <ShieldCheck className="h-5 w-5 text-white" />
          </div>
          <div>
            <h1 className="text-[15px] font-semibold text-cream-800">
              Set up Aspis Bio
            </h1>
            <p className="text-[11px] text-cream-400">
              {step === "identity"
                ? "Step 1 of 2 — your device identity"
                : step === "grant"
                  ? "Step 2 of 2 — your access grant"
                  : "All set"}
            </p>
          </div>
        </div>

        {error && (
          <div className="mb-4 rounded-xl border border-coral/20 bg-coral/10 px-3 py-2 text-[12px] text-coral-dark">
            {error}
          </div>
        )}

        {step === "identity" && (
          <div className="space-y-3">
            <p className="text-[12px] leading-5 text-cream-500">
              Create this device's identity, then send the join request to your
              admin. They approve it, pick your role, and send back a grant.
            </p>
            <button
              type="button"
              onClick={() => void createIdentity()}
              disabled={busy}
              className="inline-flex items-center gap-2 rounded-lg bg-teal px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
            >
              <Fingerprint className="h-3.5 w-3.5" />
              {joinRequest ? "Identity ready" : "Create device identity"}
            </button>

            {joinRequest && (
              <>
                <textarea
                  value={joinRequest}
                  readOnly
                  rows={6}
                  className="w-full resize-none rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[11px] text-cream-700 outline-none"
                />
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    onClick={() => void copyJoinRequest()}
                    className="inline-flex items-center gap-2 rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:text-terracotta"
                  >
                    <Copy className="h-3.5 w-3.5" />
                    {copied ? "Copied" : "Copy join request"}
                  </button>
                  <button
                    type="button"
                    onClick={() => setStep("grant")}
                    className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white"
                  >
                    I sent it — next
                  </button>
                </div>
              </>
            )}
          </div>
        )}

        {step === "grant" && (
          <div className="space-y-3">
            <p className="text-[12px] leading-5 text-cream-500">
              Paste the signed grant your admin sent back. Your app verifies it
              against the bundled trust anchor and opens in your role.
            </p>
            <textarea
              value={grant}
              onChange={(event) => setGrant(event.target.value)}
              placeholder="Paste the role grant JSON"
              rows={7}
              className="w-full resize-none rounded-lg border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[11px] text-cream-700 outline-none focus:border-terracotta-200"
            />
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => setStep("identity")}
                className="rounded-lg border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600"
              >
                Back
              </button>
              <button
                type="button"
                onClick={() => void adoptGrant()}
                disabled={busy || !grant.trim()}
                className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white disabled:opacity-60"
              >
                <ShieldCheck className="h-3.5 w-3.5" />
                Verify &amp; continue
              </button>
            </div>
          </div>
        )}

        {step === "done" && (
          <div className="space-y-3">
            <p className="text-[12px] leading-5 text-cream-600">
              You are set up as{" "}
              <span className="font-semibold">{adopted?.role ?? "collaborator"}</span>
              . Next, open <span className="font-semibold">Workspace</span> to
              pick your Aspis Bio folder or download it from the cloud.
            </p>
            <button
              type="button"
              onClick={() => void enterApp()}
              className="inline-flex items-center gap-2 rounded-lg bg-terracotta px-3 py-2 text-[12px] font-semibold text-white"
            >
              Enter Aspis
            </button>
          </div>
        )}

        {step !== "done" && (
          <div className="mt-5 border-t border-cream-100 pt-3">
            <button
              type="button"
              onClick={onSkip}
              className="text-[11px] font-medium text-cream-400 underline-offset-2 hover:text-cream-600 hover:underline"
            >
              Can't get a grant yet? Continue with limited access →
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
