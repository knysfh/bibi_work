/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { Steps } from '@arco-design/web-react';
import type { StepsProps } from '@arco-design/web-react/es/Steps';
import classNames from 'classnames';
import React from 'react';

/**
 * 步骤条组件属性 / Steps component props
 */
export interface BiWorkStepsProps extends StepsProps {
  /** 额外的类名 / Additional class name */
  className?: string;
}

/**
 * 步骤条组件 / Steps component
 *
 * 基于 Arco Design Steps 的封装，提供统一的样式主题
 * Wrapper around Arco Design Steps with unified theme styling
 *
 * @features
 * - 自定义品牌色主题 / Custom brand color theme
 * - 完成态的特殊样式处理 / Special styling for finished state
 * - 完整的 Arco Steps API 支持 / Full Arco Steps API support
 *
 * @example
 * ```tsx
 * // 基本用法 / Basic usage
 * <BiWorkSteps current={1}>
 *   <BiWorkSteps.Step title="步骤1" description="这是描述" />
 *   <BiWorkSteps.Step title="步骤2" description="这是描述" />
 *   <BiWorkSteps.Step title="步骤3" description="这是描述" />
 * </BiWorkSteps>
 *
 * // 垂直步骤条 / Vertical steps
 * <BiWorkSteps current={1} direction="vertical">
 *   <BiWorkSteps.Step title="步骤1" description="描述" />
 *   <BiWorkSteps.Step title="步骤2" description="描述" />
 * </BiWorkSteps>
 *
 * // 带图标的步骤条 / Steps with icons
 * <BiWorkSteps current={1}>
 *   <BiWorkSteps.Step title="完成" icon={<IconCheck />} />
 *   <BiWorkSteps.Step title="进行中" icon={<IconLoading />} />
 *   <BiWorkSteps.Step title="待处理" icon={<IconClock />} />
 * </BiWorkSteps>
 *
 * // 迷你版步骤条 / Mini steps
 * <BiWorkSteps current={1} size="small" type="dot">
 *   <BiWorkSteps.Step title="步骤1" />
 *   <BiWorkSteps.Step title="步骤2" />
 *   <BiWorkSteps.Step title="步骤3" />
 * </BiWorkSteps>
 * ```
 *
 * @see arco-override.css for custom styles (.biwork-steps)
 */
const BiWorkSteps: React.FC<BiWorkStepsProps> & { Step: typeof Steps.Step } = ({ className, ...props }) => {
  return <Steps {...props} className={classNames('biwork-steps', className)} />;
};

BiWorkSteps.displayName = 'BiWorkSteps';

// 导出子组件 / Export sub-component
BiWorkSteps.Step = Steps.Step;

export default BiWorkSteps;
