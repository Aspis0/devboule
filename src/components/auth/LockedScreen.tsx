import { Fingerprint } from "lucide-react";

interface LockedScreenProps {
  onUnlock: () => Promise<void>;
  isLoading?: boolean;
  unlockRetryBlocked?: boolean;
  error?: string | null;
  desktopRuntimeAvailable?: boolean;
  helloAvailable?: boolean | null;
}

export function LockedScreen({
  onUnlock,
  isLoading = false,
  unlockRetryBlocked = false,
  error,
  desktopRuntimeAvailable = true,
  helloAvailable = null,
}: LockedScreenProps) {
  const helloUnavailable = desktopRuntimeAvailable && helloAvailable === false;
  const unlockDisabled = isLoading || unlockRetryBlocked || !desktopRuntimeAvailable || helloUnavailable;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-100">
      <div className="text-center">
        <div className="mx-auto mb-6 h-24 w-24 overflow-hidden rounded-[28px] border border-cream-200 bg-white shadow-soft-md">
          <img
            src="/assets/aspis-logo.jpg"
            alt="Aspis Bio"
            className="h-full w-full object-cover"
          />
        </div>

        <h1 className="text-xl font-semibold text-cream-800 mb-1">
          Aspis Bio
        </h1>
        <p className="text-[13px] text-cream-500 mb-8">
          Infrastructure Management
        </p>

        <button
          type="button"
          onClick={(event) => {
            event.currentTarget.blur();
            void onUnlock();
          }}
          disabled={unlockDisabled}
          data-help-title="Device unlock protects the Aspis Bio control app."
          data-help-lines="This app can store cloud tokens, launch agents, and operate real infrastructure, so entry should require local device authentication.|Unlock uses Windows Hello on Windows and Touch ID on macOS — PIN, face, or fingerprint depending on your setup.|If biometric unlock loops or fails, the system PIN/password is the safer fallback path.|Unlock does not start cloud writes by itself; it only opens the management dashboard."
          className="inline-flex items-center gap-2.5 px-6 py-3 rounded-2xl
                     bg-terracotta text-white text-[14px] font-medium
                     hover:bg-terracotta-500 active:bg-terracotta-600
                     focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/40 focus-visible:ring-offset-2
                     transition-all duration-200 shadow-soft-sm hover:shadow-soft-md
                     disabled:cursor-not-allowed disabled:opacity-70"
        >
          <Fingerprint className="w-5 h-5" />
          {isLoading
            ? "Waiting for device authentication..."
            : unlockRetryBlocked
              ? "Retry in a moment"
            : !desktopRuntimeAvailable
              ? "Open the desktop app"
              : helloUnavailable
              ? "Device authentication unavailable"
              : "Unlock"}
        </button>

        {(error || !desktopRuntimeAvailable || helloUnavailable) && (
          <p className="max-w-sm text-[12px] leading-5 text-coral-dark mt-4">
            {error ||
              (!desktopRuntimeAvailable
                ? "Browser preview cannot access Tauri or device authentication. Launch Aspis Management as the desktop app."
                : "Set up device authentication (Windows Hello or Touch ID) in your system settings before using this app.")}
          </p>
        )}

        <p className="text-[11px] text-cream-400 mt-4">
          Protected by Windows Hello PIN, face, or fingerprint
        </p>
      </div>
    </div>
  );
}
