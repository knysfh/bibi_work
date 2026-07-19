/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

/**
 * BiWork 基础组件库统一导出 / BiWork base components unified exports
 *
 * 提供所有基础组件和类型的统一导出入口
 * Provides unified export entry for all base components and types
 */

// ==================== 组件导出 / Component Exports ====================

export { default as BiWorkModal } from './BiWorkModal';
export { default as BiWorkCollapse } from './BiWorkCollapse';
export { default as BiWorkSelect } from './BiWorkSelect';
export { default as BiWorkScrollArea } from './BiWorkScrollArea';
export { default as BiWorkSteps } from './BiWorkSteps';

// ==================== 类型导出 / Type Exports ====================

// BiWorkModal 类型 / BiWorkModal types
export type {
  ModalSize,
  ModalHeaderConfig,
  ModalFooterConfig,
  ModalContentStyleConfig,
  BiWorkModalProps,
} from './BiWorkModal';
export { MODAL_SIZES } from './BiWorkModal';

// BiWorkCollapse 类型 / BiWorkCollapse types
export type { BiWorkCollapseProps, BiWorkCollapseItemProps } from './BiWorkCollapse';

// BiWorkSelect 类型 / BiWorkSelect types
export type { BiWorkSelectProps } from './BiWorkSelect';

// BiWorkSteps 类型 / BiWorkSteps types
export type { BiWorkStepsProps } from './BiWorkSteps';
