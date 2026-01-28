// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useState } from 'react';

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

const PluginsView: React.FC = () => {
  const { role, isAdmin } = usePermissions();
  const [activeTab, setActiveTab] = useState<'installed' | 'marketplace'>('installed');

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
              onValueChange={(value) => setActiveTab(value as typeof activeTab)}
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
