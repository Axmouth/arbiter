import { createRouter, RouterProvider } from "@tanstack/react-router";
import { jobsRoute } from "./routes/jobs";
import { runsRoute } from "./routes/runs";
import { rootRoute } from "./routes/root";
import { workersRoute } from "./routes/workers";

const routeTree = rootRoute.addChildren([jobsRoute, runsRoute, workersRoute]);

// eslint-disable-next-line react-refresh/only-export-components
export const router = createRouter({ routeTree });
export function AppRouter() {
  return <RouterProvider router={router} />;
}
