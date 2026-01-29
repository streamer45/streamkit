// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';
import { useNavigate, useParams } from 'react-router-dom';

import { TabsContent, TabsList, TabsRoot, TabsTrigger } from '@/components/ui/Tabs';
import { usePermissions } from '@/hooks/usePermissions';

import AdminNav from './admin/AdminNav';
import InstalledPluginsTab from './plugins/InstalledPluginsTab';
import MarketplaceTab from './plugins/MarketplaceTab';
import {
  BottomSpacer,
  Card,
  Container,
  ContentArea,
  ContentWrapper,
  ErrorBox,
  Row,
  Subtle,
  Title,
  TitleRow,
} from './PluginsView.styles';

type PluginsTab = 'installed' | 'marketplace';

const isValidTab = (tab: string | undefined): tab is PluginsTab =>
  tab === 'installed' || tab === 'marketplace';

const PluginsView: React.FC = () => {
  const { role, isAdmin } = usePermissions();
  const { tab } = useParams<{ tab: string }>();
  const navigate = useNavigate();
  const activeTab: PluginsTab = isValidTab(tab) ? tab : 'installed';

  return (
    <Container>
      <ContentArea>
        <ContentWrapper>
          <Card>
            <TitleRow>
              <div>
                <Title>Plugins</Title>
                <Subtle>Manage installed plugins and marketplace installs.</Subtle>
              </div>
              <Row>
                <Subtle>Role: {role ?? 'unknown'}</Subtle>
              </Row>
            </TitleRow>

            {isAdmin() === false && (
              <ErrorBox>Admin role required to manage plugins on this server.</ErrorBox>
            )}

            <AdminNav />

            <TabsRoot
              value={activeTab}
              onValueChange={(value) => navigate(`/admin/plugins/${value}`, { replace: true })}
            >
              <TabsList>
                <TabsTrigger value="installed">Installed</TabsTrigger>
                <TabsTrigger value="marketplace">Marketplace</TabsTrigger>
              </TabsList>

              <TabsContent value="installed">
                <InstalledPluginsTab />
              </TabsContent>
              <TabsContent value="marketplace">
                <MarketplaceTab active={activeTab === 'marketplace'} />
              </TabsContent>
            </TabsRoot>
          </Card>
        </ContentWrapper>
      </ContentArea>
      <BottomSpacer />
    </Container>
  );
};

export default PluginsView;
