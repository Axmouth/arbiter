import { useState } from "react";
import { useJobs } from "../hooks/useJobs";
import { SlideOver } from "../components/SlideOver";
import type { JobSpec } from "../backend-types/JobSpec";
import { JobForm } from "../components/JobForm";
import { JobDetailsView } from "./JobDetail";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { deleteJob, disableJob, enableJob, runJobNow } from "../api/jobs";

export function JobsPage() {
  const { data: jobs, isLoading, error } = useJobs();
  const [selectedJob, setSelectedJob] = useState<JobSpec | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editMode, setEditMode] = useState(false);
  const qc = useQueryClient();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const runNowMutation = useMutation({
    mutationFn: () => runJobNow(selectedJob!.id, { command_override: null }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => deleteJob(selectedJob!.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["jobs"] });
      setDetailsOpen(false);
    },
  });

  const toggleEnabledMutation = useMutation({
  mutationFn: () =>
    selectedJob!.enabled
      ? disableJob(selectedJob!.id)
      : enableJob(selectedJob!.id),
  onSuccess: () => {
    qc.invalidateQueries({ queryKey: ["jobs"] });
    if (selectedJob) {
      setSelectedJob(prev => prev ? { ...prev, enabled: !prev.enabled } : prev);
    }
  }
});


  return (
    <div className="space-y-6">
      <h2 className="text-2xl font-semibold">Jobs</h2>

      <button
        onClick={() => setCreateOpen(true)}
        className="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700"
      >
        New Job
      </button>


      {isLoading && <div>Loading…</div>}
      {error && <div className="text-red-600">{String(error)}</div>}

      {jobs && (
        <div className="rounded-lg shadow border border-gray-200 overflow-hidden bg-white">
          <table className="w-full text-left">
            <thead className="bg-gray-50 text-gray-700">
              <tr>
                <th className="px-4 py-2 font-semibold">Name</th>
                <th className="px-4 py-2 font-semibold">Enabled</th>
                <th className="px-4 py-2 font-semibold">Cron</th>
              </tr>
            </thead>

            <tbody className="divide-y divide-gray-200">
              {jobs.map(job => (
                <tr
                  key={job.id}
                  className="hover:bg-gray-50 cursor-pointer"
                  onClick={() => {
                    setSelectedJob(job);
                    setDetailsOpen(true);
                  }}
                >
                  <td className="px-4 py-2">{job.name}</td>
                  <td className="px-4 py-2">
                    <span className={
                      job.enabled
                        ? "inline-block bg-green-100 text-green-700 px-2 py-1 text-xs rounded"
                        : "inline-block bg-red-100 text-red-700 px-2 py-1 text-xs rounded"
                    }>
                      {job.enabled ? "enabled" : "disabled"}
                    </span>
                  </td>
                  <td className="px-4 py-2">{job.schedule_cron ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <SlideOver
        open={detailsOpen}
        onClose={() => {
          setDetailsOpen(false);
          setEditMode(false);
          setCreateOpen(false);
        }}
        title={selectedJob?.name ?? ""}
      >
        {editMode ? (
          <JobForm
            mode="edit"
            initial={selectedJob!}
            onComplete={(job: JobSpec) => {
              setSelectedJob(job);

            }}
            onCancel={() => {setEditMode(false)
              setCreateOpen(false);
            }}
          />
        ) : (
          <JobDetailsView
            job={selectedJob!}
            onEdit={() => setEditMode(true)}
            onToggleEnabled={() => {
              toggleEnabledMutation.mutate();
                qc.invalidateQueries({ queryKey: ["jobs"] });
            }}
            onRunNow={() => {
              runNowMutation.mutate()
            }}
            onDelete={() => {
              if (confirm("Delete this job?")) {
                deleteMutation.mutate();
                setDetailsOpen(false);
                qc.invalidateQueries({ queryKey: ["jobs"] });
              };
            }
            }
            onComplete={() => {
              setDetailsOpen(false);
              setEditMode(false);
            }}
          />
        )}

      </SlideOver>

      <SlideOver
        open={createOpen}
        onClose={() => {
          setCreateOpen(false);
          setEditMode(false);
        }}
        title="Create Job"
      >
        <JobForm mode="create" onComplete={() => {
          setCreateOpen(false)
          setEditMode(false)
          }} onCancel={() => setCreateOpen(false)} />
      </SlideOver>

    </div>
  );
}
