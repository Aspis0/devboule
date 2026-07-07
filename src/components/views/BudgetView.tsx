import {
  AlertTriangle,
  Boxes,
  Cloud,
  Cpu,
  Database,
  HardDrive,
  Server,
  Wallet,
  Zap,
} from "lucide-react";
import { useAppContext } from "../../context/AppContext";

const resourceTypes = [
  { id: "GPU", label: "GPU", icon: Server },
  { id: "CPU VM", label: "CPU VM", icon: Cpu },
  { id: "Serverless", label: "Serverless", icon: Zap },
];

const objectStoragePrices = [
  { label: "Standard Multi-AZ", price: "€0.000020/GB/hour", monthly: "≈ €0.0146/GB/month" },
  { label: "Standard One Zone", price: "€0.0000103/GB/hour", monthly: "≈ €0.00752/GB/month" },
  { label: "Glacier", price: "€0.0000035/GB/hour", monthly: "≈ €0.00254/GB/month" },
];

const cloudflareWorkersPrices = [
  { label: "Workers Standard base", included: "10M requests + 30M CPU-ms/month", price: "$5/month" },
  { label: "Extra requests", included: "After 10M included", price: "$0.30/million" },
  { label: "Extra CPU time", included: "After 30M CPU-ms included", price: "$0.02/million CPU-ms" },
  { label: "Workers Logs", included: "20M events/month included", price: "$0.60/million extra" },
];

const cloudflareAdjacentPrices = [
  { label: "Workers KV reads", included: "10M/month included", price: "$0.50/million extra" },
  { label: "Workers KV writes", included: "1M/month included", price: "$5.00/million extra" },
  { label: "KV stored data", included: "1 GB included", price: "$0.50/GB-month extra" },
  { label: "R2 Standard storage", included: "10 GB-month free tier", price: "$0.015/GB-month" },
  { label: "R2 Class A ops", included: "1M/month free tier", price: "$4.50/million" },
  { label: "R2 Class B ops", included: "10M/month free tier", price: "$0.36/million" },
];

function formatEur(value: number) {
  return new Intl.NumberFormat("it-IT", {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: value >= 10 ? 2 : 3,
  }).format(value);
}

function formatGb(value: number) {
  return `${new Intl.NumberFormat("it-IT", {
    maximumFractionDigits: value >= 10 ? 1 : 2,
  }).format(value)} GB`;
}

export function BudgetView() {
  const { cloudSnapshot, syncProviderInventory, isLoading } = useAppContext();
  const resources = cloudSnapshot?.compute ?? [];
  const storage = cloudSnapshot?.storage ?? [];
  const workers = cloudSnapshot?.workers ?? [];
  const idleRisks = resources.filter((resource) => resource.idleCostRisk);
  const billableStorage = storage.filter((item) => item.billable);
  const objectBucketCount = storage.filter((item) => item.storageType === "Object Bucket").length;
  const scalewayStorageMonthly = storage.reduce(
    (sum, item) => sum + (item.estimatedEurMonth ?? 0),
    0,
  );
  const cloudflareMonthlyEstimate = 0;
  const totalMonthlyEstimate = scalewayStorageMonthly + cloudflareMonthlyEstimate;
  const totalStorageGb = storage.reduce((sum, item) => sum + item.sizeGb, 0);

  return (
    <div className="max-w-6xl space-y-5">
      <div className="rounded-2xl border border-amber/20 bg-white p-5">
        <div className="flex items-start gap-4">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-amber/10">
            <Wallet className="h-5 w-5 text-amber" />
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-[14px] font-semibold text-cream-800">
              Budget estimate from live inventory
            </p>
            <p className="mt-1 text-[12px] leading-5 text-cream-500">
              Scaleway storage uses public GB-hour pricing plus bounded Object Storage scans.
              Cloudflare pricing is shown as a planning reference until live usage metrics are
              connected.
            </p>
          </div>
          <button
            onClick={() => void syncProviderInventory()}
            disabled={isLoading}
            data-help-title="This refreshes budget inputs from live providers."
            data-help-lines="Budget estimates are only as good as the latest Cloudflare and Scaleway sync.|It reads inventory and pricing signals where wired.|It does not stop resources or change cloud billing by itself.|If GPU or VM costs look high, go to Compute before acting."
            className="shrink-0 rounded-xl border border-cream-200 px-3 py-2 text-[12px] font-medium text-cream-600 hover:border-terracotta-200 hover:text-terracotta disabled:opacity-60"
          >
            {isLoading ? "Syncing..." : "Sync"}
          </button>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div
          className="rounded-2xl border border-cream-200 bg-white p-5 lg:col-span-1"
          data-help-title="Unified monthly signal is the rough cost headline."
          data-help-lines="This is a planning number from the live inventory, not a provider invoice.|For Devboule, use it to spot storage or compute drift before GPU/VM work gets expensive.|Cloudflare live usage metrics are not fully connected yet.|Treat it as a warning layer, then verify in Scaleway or Cloudflare billing."
        >
          <div className="mb-4 flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-teal/10">
              <Wallet className="h-5 w-5 text-teal" />
            </div>
            <div>
              <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-400">
                Cloudflare + Scaleway
              </p>
              <p className="text-[12px] text-cream-500">Unified monthly signal</p>
            </div>
          </div>
          <p className="text-4xl font-semibold tabular-nums text-cream-800">
            {formatEur(totalMonthlyEstimate)}
          </p>
          <p className="mt-2 text-[12px] leading-5 text-cream-500">
            Current estimate includes live Scaleway storage estimates. Compute and Cloudflare usage
            need provider billing metrics before totals can be trusted as invoices.
          </p>
        </div>

        <div
          className="rounded-2xl border border-cream-200 bg-white p-5"
          data-help-title="Cloudflare budget card is planning-only for now."
          data-help-lines="This card prepares Worker, R2, KV, and adjacent Cloudflare pricing context.|For Devboule, it helps estimate edge and storage cost before enabling heavier usage.|It is not invoice-grade until live Cloudflare billing metrics are wired.|Use Cloudflare console billing for final confirmation."
        >
          <div className="mb-4 flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-orange-50">
              <Cloud className="h-5 w-5 text-orange-500" />
            </div>
            <div>
              <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-400">
                Cloudflare
              </p>
              <p className="text-[12px] text-cream-500">{workers.length} Workers synced</p>
            </div>
          </div>
          <p className="text-3xl font-semibold tabular-nums text-cream-800">
            {formatEur(cloudflareMonthlyEstimate)}
          </p>
          <p className="mt-2 text-[12px] leading-5 text-cream-500">
            Workers paid pricing is prepared below; live Cloudflare usage and billing metrics are
            not connected yet.
          </p>
        </div>

        <div
          className="rounded-2xl border border-cream-200 bg-white p-5"
          data-help-title="Scaleway budget card uses live inventory signals."
          data-help-lines="This card summarizes synced compute and storage resources from the pinned Scaleway project.|For Devboule, it is the main early warning for idle GPU/CPU VM and storage spend.|Storage estimates are closer than compute totals because VM billing metrics are not full invoices here.|Go to Compute before stopping, terminating, or deleting resources."
        >
          <div className="mb-4 flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-sage/10">
              <Database className="h-5 w-5 text-sage-dark" />
            </div>
            <div>
              <p className="text-[11px] font-semibold uppercase tracking-widest text-cream-400">
                Scaleway
              </p>
              <p className="text-[12px] text-cream-500">
                {resources.length} compute, {storage.length} storage
              </p>
            </div>
          </div>
          <p className="text-3xl font-semibold tabular-nums text-cream-800">
            {formatEur(scalewayStorageMonthly)}
          </p>
          <p className="mt-2 text-[12px] leading-5 text-cream-500">
            {formatGb(totalStorageGb)} measured Block/Snapshot storage, {objectBucketCount} bucket
            {objectBucketCount === 1 ? "" : "s"} synced.
          </p>
        </div>
      </div>

      <div className="rounded-2xl border border-cream-200 bg-white p-5">
        <div className="mb-4 flex items-center justify-between gap-3">
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Cloudflare Paid Planning
            </h3>
            <p className="mt-1 text-[12px] text-cream-400">
              Price cards for the Workers stack you may enable later.
            </p>
          </div>
          <span className="rounded-full bg-orange-50 px-3 py-1 text-[11px] font-medium text-orange-500">
            Planning only
          </span>
        </div>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
          {cloudflareWorkersPrices.map((item) => (
            <div
              key={item.label}
              className="rounded-xl bg-cream-50 px-3 py-3"
              data-help-title={`${item.label} is a Cloudflare Worker pricing reference.`}
              data-help-lines="This is a published pricing reference, not live usage.|For Devboule, use it before putting heavy API, analysis, or agent traffic behind Workers.|Extra requests and CPU time can matter if pipelines call edge endpoints often.|Confirm exact billing in Cloudflare before treating this as invoice-grade."
            >
              <p className="text-[12px] font-semibold text-cream-800">{item.label}</p>
              <p className="mt-1 text-[11px] text-cream-400">{item.included}</p>
              <p className="mt-2 font-mono text-[13px] text-cream-800">{item.price}</p>
            </div>
          ))}
        </div>
        <div className="mt-3 grid grid-cols-1 gap-2 md:grid-cols-3">
          {cloudflareAdjacentPrices.map((item) => (
            <div
              key={item.label}
              className="flex items-center justify-between gap-3 rounded-xl border border-cream-100 px-3 py-2"
              data-help-title={`${item.label} is adjacent Cloudflare pricing context.`}
              data-help-lines="R2, KV, and related operations can become hidden cost when Workers read/write a lot.|For Devboule, this matters for pipeline artifacts, cache data, queues, and agent-visible storage.|This row is a planning hint, not live usage.|Connect billing metrics before using it as a hard budget gate."
            >
              <div className="min-w-0">
                <p className="truncate text-[12px] font-medium text-cream-700">{item.label}</p>
                <p className="text-[10px] text-cream-400">{item.included}</p>
              </div>
              <p className="shrink-0 font-mono text-[11px] text-cream-800">{item.price}</p>
            </div>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
        {resourceTypes.map((item) => {
          const Icon = item.icon;
          const count = resources.filter((resource) => resource.resourceType === item.id).length;

          return (
            <div
              key={item.id}
              className="rounded-2xl border border-cream-200 bg-white p-5"
              data-help-title={`${item.label} count shows live Scaleway compute inventory.`}
              data-help-lines="This count comes from the latest Scaleway sync.|For Devboule, GPU and CPU VM counts are important because running resources can cost real money.|A count alone does not prove whether machines are useful or idle.|Open Compute for lifecycle actions and live resource details."
            >
              <div className="mb-4 flex items-center gap-3">
                <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-cream-50">
                  <Icon className="h-4.5 w-4.5 text-cream-500" />
                </div>
                <p className="text-[12px] font-semibold uppercase tracking-wider text-cream-400">
                  {item.label}
                </p>
              </div>
              <p className="text-3xl font-semibold tabular-nums text-cream-800">{count}</p>
              <p className="mt-1 text-[12px] text-cream-400">Live Scaleway compute</p>
            </div>
          );
        })}
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="rounded-2xl border border-cream-200 bg-white p-5">
          <h3 className="mb-4 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Scaleway Storage Cost
          </h3>
          <div className="space-y-2">
            {billableStorage.map((item) => (
              <div
                key={item.id}
                className="flex items-center justify-between gap-3 rounded-xl bg-cream-50 px-3 py-2"
                data-help-title={`${item.name} is a billable Scaleway storage item.`}
                data-help-lines="Storage can keep costing money even after a VM is gone.|For Devboule, delete unused VM disks, snapshots, and stale object artifacts after verifier approval.|Object bucket scans may be partial, so large buckets need provider-side confirmation.|Do not delete storage until you know which pipeline or model data it contains."
              >
                <div className="flex min-w-0 items-center gap-3">
                  <HardDrive className="h-4 w-4 shrink-0 text-cream-500" />
                  <div className="min-w-0">
                    <p className="truncate text-[12px] font-medium text-cream-800">
                      {item.name}
                    </p>
                  <p className="text-[11px] text-cream-400">
                    {item.storageType} · {item.region} · {formatGb(item.sizeGb)}
                  </p>
                  {item.tags.includes("partial-scan") && (
                    <p className="text-[10px] font-medium text-amber-dark">
                      Partial object scan
                    </p>
                  )}
                </div>
                </div>
                <div className="shrink-0 text-right">
                  <p className="font-mono text-[13px] text-cream-800">
                    {item.estimatedEurMonth === null
                      ? "Usage needed"
                      : formatEur(item.estimatedEurMonth)}
                  </p>
                  <p className="text-[10px] text-cream-400">{item.pricingLabel}</p>
                </div>
              </div>
            ))}
            {billableStorage.length === 0 && (
              <p className="rounded-xl bg-cream-50 px-3 py-3 text-[12px] text-cream-400">
                No live Scaleway storage synced.
              </p>
            )}
          </div>
        </div>

        <div className="rounded-2xl border border-cream-200 bg-white p-5">
          <h3 className="mb-4 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
            Object Storage Pricing
          </h3>
          <div className="space-y-2">
            {objectStoragePrices.map((item) => (
              <div
                key={item.label}
                className="flex items-center justify-between rounded-xl bg-cream-50 px-3 py-2"
                data-help-title={`${item.label} is a Scaleway Object Storage price tier.`}
                data-help-lines="Object Storage pricing depends on size, tier, operations, and retention behavior.|For Devboule, buckets may hold pipeline inputs, outputs, embeddings, or backups.|This row explains the pricing model; it does not prove current bucket usage.|Use saved object credentials and sync to see live buckets."
              >
                <div className="flex items-center gap-3">
                  <Boxes className="h-4 w-4 text-cream-500" />
                  <span className="text-[12px] font-medium text-cream-700">{item.label}</span>
                </div>
                <div className="text-right">
                  <p className="font-mono text-[12px] text-cream-800">{item.monthly}</p>
                  <p className="text-[10px] text-cream-400">{item.price}</p>
                </div>
              </div>
            ))}
          </div>
          <p className="mt-3 rounded-xl border border-amber/20 bg-amber/8 px-3 py-2 text-[11px] leading-5 text-amber-dark">
            Bucket names and bounded object-size scans are live when a Scaleway access key is saved
            in Secrets. Very large buckets are marked partial instead of silently pretending to be
            invoice-grade.
          </p>
        </div>
      </div>

      <div className="rounded-2xl border border-cream-200 bg-white p-5">
        <h3 className="mb-4 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Cost Risk Flags
        </h3>
        <div className="space-y-2">
          {idleRisks.map((resource) => (
            <div
              key={resource.id}
              className="flex items-center gap-3 rounded-xl bg-coral/5 px-3 py-2"
              data-help-title={`${resource.name} is flagged as a cost risk.`}
              data-help-lines="Cost risk means the resource may be running or reserved without clear useful work.|For Devboule, idle GPU/CPU VMs should be stopped or deleted quickly after confirming disks and project evidence.|Do not terminate blindly if the machine holds unsaved analysis output.|A verifier should confirm cleanup for expensive resources."
            >
              <AlertTriangle className="h-4 w-4 shrink-0 text-coral" />
              <div className="min-w-0">
                <p className="truncate text-[12px] font-medium text-cream-800">
                  {resource.name}
                </p>
                <p className="text-[11px] text-coral">
                  {resource.resourceType === "Serverless"
                    ? `Serverless min scale ${resource.minScale ?? "?"}`
                    : "Running resource marked as idle"}
                </p>
              </div>
            </div>
          ))}
          {idleRisks.length === 0 && (
            <p className="rounded-xl bg-cream-50 px-3 py-3 text-[12px] text-cream-400">
              No live idle-cost risk flags.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
