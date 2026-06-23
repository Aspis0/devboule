import { useState, type KeyboardEvent } from "react";
import { Sparkles, Send, FolderGit2 } from "lucide-react";

interface Coder {
  id: string;
  label: string;
  hint?: string;
}

interface Props {
  projectName: string | null;
  hasRoot: boolean;
  language: string | null;
  plannerModel: string;
  coders: Coder[];
  busy?: boolean;
  onPlan?: (goal: string, coderId: string, autoCreate: boolean) => void;
}

/**
 * The centerpiece "talk to the Orchestrator" composer of the Projects page: describe a goal, the
 * Orchestrator drafts a plan and the tasks land on the board. This component only DISPLAYS props +
 * emits intent (`onPlan`); all backend wiring lives in the parent (ProjectsView).
 */
export function OrchestratorHeroCard(props: Props) {
  const {
    projectName,
    hasRoot,
    language,
    plannerModel,
    coders,
    busy = false,
    onPlan,
  } = props;

  const [goal, setGoal] = useState("");
  const [coderId, setCoderId] = useState(coders.length > 0 ? coders[0].id : "");
  const [autoCreate, setAutoCreate] = useState(true);

  const submit = () => {
    const g = goal.trim();
    if (!g || !onPlan || !hasRoot || busy) return;
    onPlan(g, coderId, autoCreate);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  const isDisabled = !onPlan || !hasRoot || !goal.trim() || busy;

  return (
    <div className="rounded-2xl border border-cream-200 bg-white p-5 shadow-sm">
      {/* HEADER */}
      <div className="flex items-center gap-3">
        <div className="h-10 w-10 flex items-center justify-center rounded-xl bg-terracotta/10 text-terracotta">
          <Sparkles className="h-5 w-5" />
        </div>
        <div className="flex-1">
          <div className="text-[17px] font-bold text-cream-800">
            What should we build?
          </div>
          <div className="text-[13px] text-cream-500 mt-0.5">
            The Orchestrator drafts a complete plan, you choose who builds it,
            and the tasks are created on the board automatically.
          </div>
        </div>
        <div className="flex flex-col items-end gap-1.5">
          {language && (
            <span className="rounded-lg bg-terracotta/10 px-2.5 py-1 text-[11px] font-semibold text-terracotta">
              {language}
            </span>
          )}
          <span
            className={`rounded-lg px-2.5 py-1 text-[11px] font-semibold border ${
              hasRoot
                ? "bg-emerald-50 text-emerald-700 border-emerald-200"
                : "bg-cream-100 text-cream-500 border-cream-200"
            }`}
          >
            {hasRoot ? "● context loaded" : "○ no project root"}
          </span>
        </div>
      </div>

      {/* COMPOSER */}
      <div className="mt-4 rounded-xl border border-cream-200 bg-cream-50 p-3">
        <textarea
          rows={3}
          maxLength={2000}
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="e.g. Add Stripe billing with usage metering and a customer portal…"
          className="w-full resize-none border-none bg-transparent text-[15px] text-cream-800 outline-none leading-relaxed placeholder:text-cream-400"
        />
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <span
            title="The selected project — the goal is planned against this project."
            className="inline-flex items-center gap-1.5 rounded-lg border border-cream-200 bg-white px-2.5 py-1 text-[11px] text-cream-600"
          >
            <FolderGit2 className="h-3 w-3 text-cream-400" />
            {projectName ?? "no project selected"}
          </span>
          <span
            title="The local model that drafts the plan (configured in Settings)."
            className="rounded-lg border border-cream-200 bg-white px-2.5 py-1 text-[11px] text-cream-600"
          >
            Planner: {plannerModel}
          </span>
          <button
            type="button"
            onClick={() => setAutoCreate((v) => !v)}
            title={
              autoCreate
                ? "On: when you approve the orchestrator's plan, its tasks are added to the board automatically (you still approve the plan first)."
                : "Off: the orchestrator drafts + submits the plan but does NOT create its tasks on approval — you create them. Enforced via DEVBOULE_AUTO_CREATE."
            }
            className={`rounded-lg border px-2.5 py-1 text-[11px] font-medium transition-colors ${
              autoCreate
                ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                : "border-cream-200 bg-white text-cream-500 hover:bg-cream-50"
            }`}
          >
            auto-create tasks: {autoCreate ? "on" : "off"}
          </button>
          <button
            type="button"
            onClick={submit}
            disabled={isDisabled}
            className="ml-auto rounded-xl bg-terracotta px-4 py-2 text-[13px] font-semibold text-white inline-flex items-center gap-2 disabled:opacity-50"
          >
            <Send className="h-4 w-4" />
            {busy ? "Planning…" : "Plan it"}
          </button>
        </div>
      </div>

      {/* GUARD */}
      {!hasRoot && (
        <div className="mt-3 text-[11px] text-cream-400">
          Select a project with a working folder, then describe the goal to plan
          it.
        </div>
      )}

      {/* HAND OFF TO */}
      <div className="mt-4 rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 flex flex-wrap items-center gap-2">
        <span className="text-[11px] font-bold tracking-wide text-terracotta">
          HAND OFF TO
        </span>
        {coders.length > 0 ? (
          coders.map((c) => (
            <button
              key={c.id}
              type="button"
              onClick={() => setCoderId(c.id)}
              className={`rounded-lg px-3 py-1.5 text-[12px] font-semibold inline-flex items-center gap-2 border transition-colors ${
                coderId === c.id
                  ? "border-terracotta bg-white text-cream-800 shadow-sm"
                  : "border-cream-200 bg-white text-cream-600 hover:bg-cream-100"
              }`}
            >
              <span className="h-5 w-5 rounded bg-terracotta/10 text-terracotta text-[10px] font-bold flex items-center justify-center">
                {c.label.charAt(0).toUpperCase()}
              </span>
              {c.label}
            </button>
          ))
        ) : (
          <span className="text-[11px] text-cream-400">
            No coders configured — add one in Settings
          </span>
        )}
      </div>
    </div>
  );
}
