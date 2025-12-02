import { createRoute } from "@tanstack/react-router";
import { rootRoute } from "./root";
import { RunsPage } from "../pages/RunsPage";

export const runsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/runs",
  component: RunsPage,
});
