import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import 'react-js-cron/dist/styles.css'
import App from './App.tsx'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

const qc = new QueryClient()

createRoot(document.getElementById('root')!).render(
  <QueryClientProvider client={qc}>
    <StrictMode>
      <App />
    </StrictMode>
  </QueryClientProvider>
)
