import { Fragment, type ReactNode } from "react";
import { Dialog, DialogPanel, DialogTitle, Transition, TransitionChild } from "@headlessui/react";

type SlideOverProps = {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  children?: ReactNode;
};

export function SlideOver({ open, onClose, title, children }: SlideOverProps) {
  return (
    <Transition show={open} as={Fragment}>
      <Dialog as="div" className="relative z-50" onClose={onClose}>
        
        {/* Dim background */}
        <TransitionChild
          as={Fragment}
          enter="ease-out duration-200" enterFrom="opacity-0" enterTo="opacity-100"
          leave="ease-in duration-150" leaveFrom="opacity-100" leaveTo="opacity-0"
        >
          <div className="fixed inset-0 bg-black/30" />
        </TransitionChild>

        {/* Right Drawer */}
        <div className="fixed inset-0 overflow-hidden">
          <div className="absolute inset-y-0 right-0 flex max-w-full pointer-events-none">

            <TransitionChild
              as={Fragment}
              enter="transform transition ease-in-out duration-300"
              enterFrom="translate-x-full" enterTo="translate-x-0"
              leave="transform transition ease-in-out duration-300"
              leaveFrom="translate-x-0" leaveTo="translate-x-full"
            >
              <DialogPanel className="pointer-events-auto w-screen max-w-md bg-white shadow-xl">
                <div className="flex flex-col h-full">

                  {/* Header */}
                  <div className="px-6 py-4 border-b flex items-center justify-between">
                    <DialogTitle className="text-lg font-medium">
                      {title}
                    </DialogTitle>

                    <button
                      onClick={onClose}
                      className="text-gray-500 hover:text-gray-700"
                    >
                      ✕
                    </button>
                  </div>

                  {/* Content */}
                  <div className="flex-1 overflow-y-auto p-6">
                    {children}
                  </div>

                </div>
              </DialogPanel>
            </TransitionChild>

          </div>
        </div>
      </Dialog>
    </Transition>
  );
}
