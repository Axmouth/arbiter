import { Fragment, type ReactNode } from 'react'
import {
  Dialog,
  DialogPanel,
  DialogTitle,
  Transition,
  TransitionChild,
} from '@headlessui/react'

type SlideOverProps = {
  open: boolean
  onClose: () => void
  title?: ReactNode
  children?: ReactNode
}

export function SlideOver({ open, onClose, title, children }: SlideOverProps) {
  return (
    <Transition show={open} as={Fragment}>
      <Dialog as="div" className="relative z-50" onClose={onClose}>
        {/* Dim background */}
        <TransitionChild
          as={Fragment}
          enter="ease-out duration-200"
          enterFrom="opacity-0"
          enterTo="opacity-100"
          leave="ease-in duration-150"
          leaveFrom="opacity-100"
          leaveTo="opacity-0"
        >
          <div className="fixed inset-0 bg-black/30" />
        </TransitionChild>

        {/* Right Drawer */}
        <div className="fixed inset-0 overflow-hidden">
          <div className="absolute inset-y-0 right-0 flex max-w-full pointer-events-none">
            <TransitionChild
              as={Fragment}
              enter="transform transition ease-in-out duration-300"
              enterFrom="translate-x-full"
              enterTo="translate-x-0"
              leave="transform transition ease-in-out duration-300"
              leaveFrom="translate-x-0"
              leaveTo="translate-x-full"
            >
              <DialogPanel
                className="
                  pointer-events-auto w-screen max-w-md shadow-xl
                  bg-(--bg-surface-dialog)
                  text-(--text-primary)
                "
              >
                <div className="flex flex-col h-full">
                  {/* Header */}
                  <div
                    className="
                        px-6 py-4 flex items-center justify-between border-b
                        bg-(--bg-surface-dialog-header)
                        border-(--border-subtle)
                        text-(--text-primary)
                      "
                  >
                    <DialogTitle className="text-lg font-medium">
                      {title}
                    </DialogTitle>

                    <button
                      onClick={onClose}
                      className="
                        text-(--text-secondary)
                        hover:text-(--text-primary)
                      "
                    >
                      ✕
                    </button>
                  </div>

                  {/* Content */}
                  <div className="flex-1 overflow-y-auto p-6 bg-(--bg-surface-dialog)">
                    {children}
                  </div>
                </div>
              </DialogPanel>
            </TransitionChild>
          </div>
        </div>
      </Dialog>
    </Transition>
  )
}
