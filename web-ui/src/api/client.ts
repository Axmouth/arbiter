import type { ApiResponse } from "../backend-types/ApiResponse";

export async function api<T>(path: string, options: RequestInit = {}, query: Record<string, string> = {}): Promise<T> {
  const versionedPath = path.startsWith("/api/v1") ? path : `/api/v1${path}`;

  // Add localhost:8080 for development
  const apiUrl = import.meta.env.MODE === "development1" ? `http://localhost:8080${versionedPath}` : versionedPath;
  const paramsStr = new URLSearchParams(query as Record<string, string>).toString();
  const urlWithQuery = paramsStr ? `${apiUrl}?${paramsStr}` : apiUrl;
  console.log("API Request:", versionedPath, options);
  const res = await fetch(urlWithQuery, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });

  const json: ApiResponse<T> = await res.json();
  console.log("API Response:", path, json);

  // Unauthorized
  if (res.status === 401) {
    // navigate({ to: "/" });
  }

  if (json.status === "error") {
    console.error("API Error:", versionedPath, json.message);
    throw new Error(json.message);
  }

  return json.data as T;
}
