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
          data-help-title="Windows Hello unlock protects the Aspis Bio control app."
          data-help-lines="This app can store cloud tokens, launch agents, and operate real infrastructure, so entry should require local Windows authentication.|Windows Hello can use PIN, face, or fingerprint depending on your Windows setup.|If camera unlock loops or fails, PIN is the safer fallback path until the Hello issue is fixed.|Unlock does not start cloud writes by itself; it only opens the management dashboard."
          className="inline-flex items-center gap-2.5 px-6 py-3 rounded-2xl
                     bg-terracotta text-white text-[14px] font-medium
                     hover:bg-terracotta-500 active:bg-terracotta-600
                     focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/40 focus-visible:ring-offset-2
                     transition-all duration-200 shadow-soft-sm hover:shadow-soft-md
                     disabled:cursor-not-allowed disabled:opacity-70"
        >
          <Fingerprint className="w-5 h-5" />
          {isLoading
            ? "Waiting for Windows Hello..."
            : unlockRetryBlocked
              ? "Retry in a moment"
            : !desktopRuntimeAvailable
              ? "Open Windows app"
              : helloUnavailable
              ? "Windows Hello unavailable"
              : "Unlock with Windows Hello"}
        </button>

        {(error || !desktopRuntimeAvailable || helloUnavailable) && (
          <p className="max-w-sm text-[12px] leading-5 text-coral-dark mt-4">
            {error ||
              (!desktopRuntimeAvailable
                ? "Browser preview cannot access Tauri or Windows Hello. Launch Aspis Management as the Windows desktop app."
                : "Set up Windows Hello PIN, face, or fingerprint in Windows Settings before using this app.")}
          </p>
        )}

        <p className="text-[11px] text-cream-400 mt-4">
          Protected by Windows Hello PIN, face, or fingerprint
        </p>
      </div>
    </div>
  );
}
