import React from 'react';
import {Composition, useCurrentFrame} from 'remotion';
const Hello: React.FC = () => {
  const frame = useCurrentFrame();
  return <div style={{flex: 1, background: '#123', color: 'white', fontSize: 40, fontFamily: 'Arial'}}>Frame {frame}</div>;
};
export const Root: React.FC = () => (
  <>
    <Composition id="Main" component={Hello} width={320} height={180} fps={30} durationInFrames={20} />
    <Composition id="Second" component={Hello} width={640} height={360} fps={24} durationInFrames={12} />
  </>
);
