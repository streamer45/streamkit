// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import React, { Suspense } from 'react';
import { Outlet, NavLink } from 'react-router-dom';
import { useShallow } from 'zustand/shallow';

import logo from './assets/logo.png';
import { LayoutPresetButtons } from './components/LayoutPresetButtons';
import { LoadingSpinner } from './components/LoadingSpinner';
import { Button } from './components/ui/Button';
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
  FormGroup,
  Input,
  Label,
} from './components/ui/Dialog';
import { useTheme, type ColorMode } from './context/ThemeContext';
import { fetchHealth } from './services/health';
import { LAYOUT_PRESETS, useLayoutStore, type LayoutPreset } from './stores/layoutStore';
import { usePermissionStore } from './stores/permissionStore';
import { getLogger } from './utils/logger';

const logger = getLogger('Layout');

type BuildInfo = {
  version: string;
  buildHash: string;
};

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

const LogoButton = styled.button`
  display: inline-flex;
  align-items: center;
  padding: 0;
  border: none;
  background: none;
  cursor: pointer;

  &:focus-visible {
    outline: none;
    box-shadow: var(--sk-focus-ring);
    border-radius: 8px;
  }
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
  min-width: 0;
  min-height: 0;
`;

const Layout: React.FC = () => {
  const [buildInfo, setBuildInfo] = React.useState<BuildInfo | null>(null);
  const closeButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const { colorMode, setColorMode } = useTheme();
  const role = usePermissionStore((s) => s.role);
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
  const version = buildInfo?.version ?? 'unknown';
  const buildHash = buildInfo?.buildHash ?? 'unknown';
  const handleDialogOpenAutoFocus = React.useCallback((event: Event) => {
    event.preventDefault();
    closeButtonRef.current?.focus();
  }, []);

  React.useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();

    (async () => {
      try {
        const info = await fetchHealth(controller.signal);
        if (!cancelled) {
          setBuildInfo(info);
        }
      } catch (err) {
        const isAbortError = err instanceof Error && err.name === 'AbortError';
        const isAbortRelated = err instanceof DOMException && err.name === 'AbortError';
        if (!cancelled && !isAbortError && !isAbortRelated) {
          logger.debug('Failed to load build info', err);
        }
      }
    })();

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, []);

  return (
    <LayoutContainer>
      <Nav>
        <LogoContainer>
          <Dialog>
            <DialogTrigger asChild>
              <LogoButton type="button" aria-label="About StreamKit">
                <Logo src={logo} alt="StreamKit" />
              </LogoButton>
            </DialogTrigger>
            <DialogPortal>
              <DialogOverlay />
              <DialogContent onOpenAutoFocus={handleDialogOpenAutoFocus}>
                <DialogHeader>
                  <DialogTitle>About StreamKit</DialogTitle>
                  <DialogDescription>Build info for support and debugging.</DialogDescription>
                </DialogHeader>
                <DialogBody>
                  <FormGroup spacing="compact">
                    <Label htmlFor="about-version">Version</Label>
                    <Input id="about-version" readOnly value={version} />
                  </FormGroup>
                  <FormGroup spacing="compact">
                    <Label htmlFor="about-build-hash">Build hash</Label>
                    <Input id="about-build-hash" readOnly value={buildHash} />
                  </FormGroup>
                </DialogBody>
                <DialogFooter>
                  <DialogClose asChild>
                    <Button ref={closeButtonRef} variant="primary">
                      Close
                    </Button>
                  </DialogClose>
                </DialogFooter>
              </DialogContent>
            </DialogPortal>
          </Dialog>
        </LogoContainer>
        <NavLinks>
          <StyledNavLink to="/design">Design</StyledNavLink>
          <StyledNavLink to="/monitor">Monitor</StyledNavLink>
          <StyledNavLink to="/convert">Convert</StyledNavLink>
          <StyledNavLink to="/stream">Stream</StyledNavLink>
          {role === 'admin' && <StyledNavLink to="/admin">Admin</StyledNavLink>}
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
        <Suspense fallback={<LoadingSpinner message="Loading..." />}>
          <Outlet />
        </Suspense>
      </Main>
    </LayoutContainer>
  );
};

export default Layout;
