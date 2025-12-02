import { Link } from "@tanstack/react-router";

export function AppLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen bg-gray-100 text-gray-900 flex flex-col">

      <header className="h-14 bg-white border-b shadow-sm flex items-center px-6 space-x-6">
        <h1 className="text-xl font-semibold mr-4">Dromio Scheduler</h1>

        <Link to="/" className="hover:text-blue-600">Jobs</Link>
        <Link to="/runs" className="hover:text-blue-600">Runs</Link>
        <Link to="/workers" className="hover:text-blue-600">Workers</Link>
      </header>

      <main className="flex-1 p-8">
        <div className="max-w-6xl mx-auto">
          {children}
        </div>
      </main>

    </div>
  );
}
