import { RouterProvider } from "@tanstack/react-router";
import { router } from "./router";
import { AuthProvider, useAuth } from './auth/AuthContext.tsx';

function InnerApp() {
  const auth = useAuth()
  return <RouterProvider router={router} context={{ auth }} />
}

export default function App() {
    return <AuthProvider>
      <InnerApp />
    </AuthProvider>
}
