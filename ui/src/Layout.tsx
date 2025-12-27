// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import React from 'react';
import { Outlet, NavLink } from 'react-router-dom';
import { useShallow } from 'zustand/shallow';

import logo from './assets/logo.png';
import { LayoutPresetButtons } from './components/LayoutPresetButtons';
import { Button } from './components/ui/Button';
import { useTheme, type ColorMode } from './context/ThemeContext';
import { LAYOUT_PRESETS, useLayoutStore, type LayoutPreset } from './stores/layoutStore';

const LayoutContainer = styled.div`
  display: flex;
  flex-direction: column;
  height: 100vh;
`;

const Nav = styled.nav`
  display: flex;
  gap: 20px;
  padding: 8px 20px;
  background-color: var(--sk-sidebar-bg);
  border-bottom: 1px solid var(--sk-border);
  align-items: center;
  max-width: 100%;
  overflow: hidden;
  flex-wrap: wrap;

  @media (max-width: 768px) {
    gap: 12px;
    padding: 8px 12px;
  }
`;

const LogoContainer = styled.div`
  display: flex;
  align-items: center;
  user-select: none;
`;

const Logo = styled.img`
  height: 42px;
  width: auto;

  @media (max-width: 768px) {
    height: 34px;
  }
`;

const NavLinks = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
  flex: 1 1 auto;
  overflow-x: auto;
  overflow-y: hidden;
  -webkit-overflow-scrolling: touch;

  @media (max-width: 768px) {
    flex-basis: 100%;
  }
`;

const StyledNavLink = styled(NavLink)`
  padding: 8px 16px;
  border-radius: 6px;
  text-decoration: none;
  color: var(--sk-text);
  background-color: transparent;
  font-weight: 400;

  &:hover,
  &:focus-visible {
    background-color: var(--sk-hover-bg);
    outline: none;
  }

  &.active {
    color: var(--sk-primary-contrast);
    background-color: var(--sk-primary);
    font-weight: 600;
  }
`;

const NavControls = styled.div`
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 12px;

  @media (max-width: 768px) {
    margin-left: 0;
    width: 100%;
    justify-content: flex-end;
  }
`;

const DesktopNavControls = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;

  @media (max-width: 768px) {
    display: none;
  }
`;

const MobileNavControls = styled.div`
  display: none;

  @media (max-width: 768px) {
    display: flex;
    align-items: center;
    gap: 8px;
  }
`;

const StyledMenuContent = styled(DropdownMenu.Content)`
  background-color: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 10px;
  box-shadow: 0 8px 24px var(--sk-shadow);
  color: var(--sk-text);
  padding: 6px;
  min-width: 220px;
  z-index: 2000;
`;

const StyledMenuLabel = styled(DropdownMenu.Label)`
  padding: 6px 10px;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const StyledMenuSeparator = styled(DropdownMenu.Separator)`
  height: 1px;
  background-color: var(--sk-border);
  margin: 6px 2px;
`;

const StyledRadioItem = styled(DropdownMenu.RadioItem)`
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 6px;
  cursor: pointer;
  user-select: none;
  outline: none;

  &[data-highlighted] {
    background-color: var(--sk-hover-bg);
  }
`;

const ItemIndicatorSlot = styled.div`
  width: 16px;
  display: inline-flex;
  justify-content: center;
  color: var(--sk-primary);
`;

const PresetIcon = styled.span`
  width: 18px;
  display: inline-flex;
  justify-content: center;
  color: var(--sk-text-muted);
`;

const ItemLabel = styled.span`
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

const Main = styled.main`
  flex: 1;
  overflow: hidden;
`;

const Layout: React.FC = () => {
  const { colorMode, setColorMode } = useTheme();
  const { currentPreset, setPreset } = useLayoutStore(
    useShallow((state) => ({
      currentPreset: state.currentPreset,
      setPreset: state.setPreset,
    }))
  );
  const presetOrder: LayoutPreset[] = [
    'palette-focus',
    'balanced',
    'focus-canvas',
    'inspector-focus',
  ];

  return (
    <LayoutContainer>
      <Nav>
        <LogoContainer>
          <Logo src={logo} alt="StreamKit" />
        </LogoContainer>
        <NavLinks>
          <StyledNavLink to="/design">Design</StyledNavLink>
          <StyledNavLink to="/monitor">Monitor</StyledNavLink>
          <StyledNavLink to="/convert">Convert</StyledNavLink>
          <StyledNavLink to="/stream">Stream</StyledNavLink>
        </NavLinks>
        <NavControls>
          <DesktopNavControls>
            <LayoutPresetButtons />
            <select
              className="xy-theme__select"
              onChange={(e) => setColorMode(e.target.value as ColorMode)}
              value={colorMode}
              aria-label="Color mode"
            >
              <option value="dark">dark</option>
              <option value="light">light</option>
              <option value="system">system</option>
            </select>
          </DesktopNavControls>

          <MobileNavControls>
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <Button variant="icon" size="small" aria-label="Open layout and theme menu">
                  ⋯
                </Button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <StyledMenuContent sideOffset={8} align="end">
                  <StyledMenuLabel>Layout</StyledMenuLabel>
                  <DropdownMenu.RadioGroup
                    value={currentPreset}
                    onValueChange={(preset) => setPreset(preset as LayoutPreset)}
                  >
                    {presetOrder.map((presetId) => (
                      <StyledRadioItem key={presetId} value={presetId}>
                        <ItemIndicatorSlot>
                          <DropdownMenu.ItemIndicator>✓</DropdownMenu.ItemIndicator>
                        </ItemIndicatorSlot>
                        <PresetIcon aria-hidden="true">{LAYOUT_PRESETS[presetId].icon}</PresetIcon>
                        <ItemLabel>{LAYOUT_PRESETS[presetId].name}</ItemLabel>
                      </StyledRadioItem>
                    ))}
                  </DropdownMenu.RadioGroup>

                  <StyledMenuSeparator />

                  <StyledMenuLabel>Theme</StyledMenuLabel>
                  <DropdownMenu.RadioGroup
                    value={colorMode}
                    onValueChange={(mode) => setColorMode(mode as ColorMode)}
                  >
                    {(['dark', 'light', 'system'] as const).map((mode) => (
                      <StyledRadioItem key={mode} value={mode}>
                        <ItemIndicatorSlot>
                          <DropdownMenu.ItemIndicator>✓</DropdownMenu.ItemIndicator>
                        </ItemIndicatorSlot>
                        <ItemLabel>{mode}</ItemLabel>
                      </StyledRadioItem>
                    ))}
                  </DropdownMenu.RadioGroup>
                </StyledMenuContent>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
          </MobileNavControls>
        </NavControls>
      </Nav>
      <Main>
        <Outlet />
      </Main>
    </LayoutContainer>
  );
};

export default Layout;
