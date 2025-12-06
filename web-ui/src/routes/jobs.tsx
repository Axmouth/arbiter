import { createRoute } from "@tanstack/react-router";
import { rootRoute } from "./root";
import { JobsPage } from "../pages/JobsPage";

export const jobsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/jobs",
  component: JobsPage,
});
