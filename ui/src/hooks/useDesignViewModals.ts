// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState, useCallback } from 'react';

/**
 * Custom hook to manage modal states in DesignView
 */
export function useDesignViewModals() {
  const [showClearModal, setShowClearModal] = useState(false);
  const [showSaveModal, setShowSaveModal] = useState(false);
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [showLoadSampleModal, setShowLoadSampleModal] = useState(false);
  const [showSaveFragmentModal, setShowSaveFragmentModal] = useState(false);
  const [pendingSample, setPendingSample] = useState<{
    yaml: string;
    name: string;
    description: string;
  } | null>(null);

  const handleOpenClearModal = useCallback(() => {
    setShowClearModal(true);
  }, []);

  const handleCloseClearModal = useCallback(() => {
    setShowClearModal(false);
  }, []);

  const handleOpenSaveModal = useCallback(() => {
    setShowSaveModal(true);
  }, []);

  const handleCloseSaveModal = useCallback(() => {
    setShowSaveModal(false);
  }, []);

  const handleOpenCreateModal = useCallback(() => {
    setShowCreateModal(true);
  }, []);

  const handleCloseCreateModal = useCallback(() => {
    setShowCreateModal(false);
  }, []);

  const handleOpenLoadSampleModal = useCallback(() => {
    setShowLoadSampleModal(true);
  }, []);

  const handleCloseLoadSampleModal = useCallback(() => {
    setShowLoadSampleModal(false);
    setPendingSample(null);
  }, []);

  const handleOpenSaveFragmentModal = useCallback(() => {
    setShowSaveFragmentModal(true);
  }, []);

  const handleCloseSaveFragmentModal = useCallback(() => {
    setShowSaveFragmentModal(false);
  }, []);

  return {
    // State
    showClearModal,
    showSaveModal,
    showCreateModal,
    showLoadSampleModal,
    showSaveFragmentModal,
    pendingSample,
    setPendingSample,

    // Handlers
    handleOpenClearModal,
    handleCloseClearModal,
    handleOpenSaveModal,
    handleCloseSaveModal,
    handleOpenCreateModal,
    handleCloseCreateModal,
    handleOpenLoadSampleModal,
    handleCloseLoadSampleModal,
    handleOpenSaveFragmentModal,
    handleCloseSaveFragmentModal,
  };
}
