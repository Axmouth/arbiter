import { createContext, useContext, useEffect, useState } from "react";
import type { User } from "../backend-types";
import { me as fetchMe, login as apiLogin, logout as apiLogout } from "../api/auth";
import { redirect } from "@tanstack/react-router";

export type AuthState =
  | { status: "loading"; user: null }
  | { status: "unauthenticated"; user: null }
  | { status: "authenticated"; user: User };

export type AuthContextValue = {
  state: AuthState;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
};

const AuthContext = createContext<AuthContextValue | undefined>(undefined);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = useState<AuthState>({ status: "loading", user: null });

  useEffect(() => {
    fetchMe()
      .then((user) => setState({ status: "authenticated", user }))
      .catch(() => setState({ status: "unauthenticated", user: null }));
  }, []);

  async function login(username: string, password: string) {
    const user = await apiLogin(username, password);
    setState({ status: "authenticated", user });
  }

  async function logout() {
    await apiLogout();
    setState({ status: "unauthenticated", user: null });

    throw redirect({ to: "/login" });
  }

  return (
    <AuthContext.Provider value={{ state, login, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

// TODO: ?
// eslint-disable-next-line react-refresh/only-export-components
export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used inside AuthProvider");
  return ctx;
}
