// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

let turnCounter = 0;
let lastResolved = null; // { name, admin1, country, latitude, longitude }
const MAX_CONVERSATION_MESSAGES = 12; // 6 turns (user+assistant)
let conversation = [];

function pushConversation(role, content) {
  const normalized = normalizeWhitespace(content);
  if (!normalized) return;
  if (role !== 'user' && role !== 'assistant') return;
  conversation.push({ role, content: normalized });
  if (conversation.length > MAX_CONVERSATION_MESSAGES) {
    conversation = conversation.slice(conversation.length - MAX_CONVERSATION_MESSAGES);
  }
}

function normalizeWhitespace(s) {
  return String(s || '').replace(/\s+/g, ' ').trim();
}

function isFiniteNumber(value) {
  return typeof value === 'number' && Number.isFinite(value);
}

function spokenDegrees(value, unit) {
  if (!isFiniteNumber(value)) return null;
  const rounded = Math.round(value * 10) / 10;
  const u = unit === 'fahrenheit' ? 'Fahrenheit' : 'Celsius';
  return `${rounded} degrees ${u}`;
}

function spokenWind(value, unit) {
  if (!isFiniteNumber(value)) return null;
  const rounded = Math.round(value);
  const u = unit === 'mph' ? 'miles per hour' : 'kilometers per hour';
  return `${rounded} ${u}`;
}

function spokenPercent(value) {
  if (!isFiniteNumber(value)) return null;
  const rounded = Math.round(value);
  return `${rounded} percent`;
}

function clampInt(value, min, max, fallback) {
  const n = Number(value);
  if (!Number.isFinite(n)) return fallback;
  const i = Math.trunc(n);
  if (i < min) return min;
  if (i > max) return max;
  return i;
}

function normalizeTemperatureUnit(value) {
  const v = String(value || '').toLowerCase().trim();
  if (v === 'fahrenheit' || v === 'f') return 'fahrenheit';
  return 'celsius';
}

function normalizeWindUnit(value) {
  const v = String(value || '').toLowerCase().trim();
  if (v === 'mph') return 'mph';
  return 'kmh';
}

function tryExtractJsonObject(text) {
  const raw = String(text || '').trim();
  if (!raw) return null;

  try {
    return JSON.parse(raw);
  } catch (_) {}

  const fence = raw.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);
  if (fence && fence[1]) {
    try {
      return JSON.parse(fence[1]);
    } catch (_) {}
  }

  const start = raw.indexOf('{');
  const end = raw.lastIndexOf('}');
  if (start >= 0 && end > start) {
    const candidate = raw.slice(start, end + 1);
    try {
      return JSON.parse(candidate);
    } catch (_) {}
  }
  return null;
}

function describeWeatherCode(code) {
  switch (Number(code)) {
    case 0:
      return 'clear sky';
    case 1:
      return 'mostly clear';
    case 2:
      return 'partly cloudy';
    case 3:
      return 'overcast';
    case 45:
      return 'fog';
    case 48:
      return 'depositing rime fog';
    case 51:
      return 'light drizzle';
    case 53:
      return 'drizzle';
    case 55:
      return 'dense drizzle';
    case 56:
      return 'light freezing drizzle';
    case 57:
      return 'freezing drizzle';
    case 61:
      return 'light rain';
    case 63:
      return 'rain';
    case 65:
      return 'heavy rain';
    case 66:
      return 'light freezing rain';
    case 67:
      return 'freezing rain';
    case 71:
      return 'light snow';
    case 73:
      return 'snow';
    case 75:
      return 'heavy snow';
    case 77:
      return 'snow grains';
    case 80:
      return 'light rain showers';
    case 81:
      return 'rain showers';
    case 82:
      return 'violent rain showers';
    case 85:
      return 'light snow showers';
    case 86:
      return 'snow showers';
    case 95:
      return 'thunderstorm';
    case 96:
      return 'thunderstorm with hail';
    case 99:
      return 'thunderstorm with heavy hail';
    default:
      return 'unknown conditions';
  }
}

function locationLabel(loc) {
  if (!loc) return '';
  const parts = [];
  if (loc.name) parts.push(loc.name);
  if (loc.admin1) parts.push(loc.admin1);
  if (loc.country) parts.push(loc.country);
  return parts.join(', ');
}

function isFetchBlockedError(e) {
  const msg = String(e || '');
  return msg.includes('Blocked: Global allowlist is empty') || msg.includes('Blocked: URL not in global allowlist');
}

function allowlistHint(service) {
  const what = service ? ` for ${service}` : '';
  return `This server blocks fetch() calls${what}. Add it to script.global_fetch_allowlist in skit.toml, then restart skit.`;
}

async function parseRequestWithOpenAI(text, turnId) {
  const spanId = telemetry.startSpan('llm.request', {
    turn_id: turnId,
    model: 'gpt-4-turbo',
    input_chars: text.length,
    input_preview: text.slice(0, 80),
  });

  try {
    const lastLocation = lastResolved ? locationLabel(lastResolved) : 'none';
    const responseText = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      body: JSON.stringify({
        model: 'gpt-4-turbo',
        temperature: 0.0,
        max_tokens: 220,
        messages: [
          {
            role: 'system',
            content:
              'You extract structured intent from a voice request for a weather assistant.\n' +
              'Return ONLY valid JSON. No prose, no markdown.\n' +
              'Schema:\n' +
              '{\n' +
              '  "intent": "weather" | "other",\n' +
              '  "location_query": string | null,\n' +
              '  "use_last_location": boolean,\n' +
              '  "request": {\n' +
              '    "kind": "current" | "daily",\n' +
              '    "start_offset_days": 0 | 1,\n' +
              '    "days": integer\n' +
              '  },\n' +
              '  "temperature_unit": "celsius" | "fahrenheit",\n' +
              '  "wind_speed_unit": "kmh" | "mph"\n' +
              '}\n' +
              'Rules:\n' +
              '- location_query is a place name ONLY (city/region/country). Never a time phrase.\n' +
              '- If the user does not specify a location, set location_query=null.\n' +
              '- If last_location is not "none" and the user did not specify a location, set use_last_location=true.\n' +
              '- If the user specifies a new location, set use_last_location=false.\n' +
              '- "right now" => kind="current", start_offset_days=0, days=1.\n' +
              '- "today" => kind="daily", start_offset_days=0.\n' +
              '- "tomorrow" => kind="daily", start_offset_days=1.\n' +
              '- "next couple of days" / "next few days" => kind="daily", start_offset_days=0, days=3.\n' +
              '- days must be 1-7.\n' +
              '- Defaults: kind="daily", start_offset_days=0, days=1, temperature_unit=celsius, wind_speed_unit=kmh.\n',
          },
          ...conversation,
          {
            role: 'user',
            content: 'last_location: ' + lastLocation + '\n' + 'request: ' + text,
          },
        ],
      }),
    });

    const result = JSON.parse(responseText || '{}');
    const content =
      result && result.choices && result.choices[0] && result.choices[0].message && result.choices[0].message.content
        ? String(result.choices[0].message.content).trim()
        : '';

    const parsed = tryExtractJsonObject(content);
    telemetry.endSpan(spanId, {
      turn_id: turnId,
      status: parsed ? 'success' : 'invalid_json',
      output_preview: content.slice(0, 120),
    });
    return parsed;
  } catch (e) {
    telemetry.endSpan(spanId, { turn_id: turnId, status: 'error', error: String(e || 'unknown') });
    return { __error: isFetchBlockedError(e) ? allowlistHint('OpenAI') : null };
  }
}

async function speakResponseWithOpenAI(resolved, parsed, forecast, userText, turnId) {
  const spanId = telemetry.startSpan('llm.reply', {
    turn_id: turnId,
    model: 'gpt-4-turbo',
    location: locationLabel(resolved),
    kind: parsed.request.kind,
    start_offset_days: parsed.request.start_offset_days,
    days: parsed.request.days,
  });

  try {
    const loc = locationLabel(resolved);
    const req = parsed.request;
    const units = { temperature_unit: parsed.temperature_unit, wind_speed_unit: parsed.wind_speed_unit };

    const cw = forecast.current_weather || {};
    const daily = forecast.daily || {};
    const times = Array.isArray(daily.time) ? daily.time : [];
    const tMaxArr = Array.isArray(daily.temperature_2m_max) ? daily.temperature_2m_max : [];
    const tMinArr = Array.isArray(daily.temperature_2m_min) ? daily.temperature_2m_min : [];
    const popArr = Array.isArray(daily.precipitation_probability_max) ? daily.precipitation_probability_max : [];
    const codeArr = Array.isArray(daily.weathercode) ? daily.weathercode : [];

    const payload = {
      location: loc,
      request: req,
      units,
      current: {
        temperature: cw.temperature,
        windspeed: cw.windspeed,
        weathercode: cw.weathercode,
      },
      daily: {
        time: times,
        temperature_2m_max: tMaxArr,
        temperature_2m_min: tMinArr,
        precipitation_probability_max: popArr,
        weathercode: codeArr,
      },
    };

    const responseText = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      body: JSON.stringify({
        model: 'gpt-4-turbo',
        temperature: 0.6,
        max_tokens: 140,
        messages: [
          {
            role: 'system',
            content:
              'You are a voice weather assistant. Write a short, conversational answer suitable for TTS.\n' +
              'Requirements:\n' +
              '- 1 to 3 sentences.\n' +
              '- No emojis.\n' +
              '- Avoid symbols like "%" and "°" (say "percent" and "degrees").\n' +
              '- If multiple days are requested, summarize trends briefly instead of listing many numbers.\n' +
              '- Always mention the location.\n',
          },
          ...conversation,
          { role: 'user', content: userText },
          { role: 'user', content: 'Weather data JSON:\n' + JSON.stringify(payload) },
        ],
      }),
    });

    const result = JSON.parse(responseText || '{}');
    const content =
      result && result.choices && result.choices[0] && result.choices[0].message && result.choices[0].message.content
        ? String(result.choices[0].message.content).trim()
        : '';

    telemetry.endSpan(spanId, { turn_id: turnId, status: content ? 'success' : 'empty' });
    return content || null;
  } catch (e) {
    telemetry.endSpan(spanId, { turn_id: turnId, status: 'error', error: String(e || 'unknown') });
    return null;
  }
}

function normalizeParsedRequest(parsed) {
  const intent = parsed && parsed.intent ? String(parsed.intent).toLowerCase().trim() : 'other';
  const locationQuery =
    parsed && parsed.location_query ? normalizeWhitespace(parsed.location_query) : null;
  const useLast = Boolean(parsed && parsed.use_last_location);

  const req = parsed && parsed.request ? parsed.request : {};
  const kind = req && req.kind ? String(req.kind).toLowerCase().trim() : 'daily';
  const startOffsetDays = req ? clampInt(req.start_offset_days, 0, 1, 0) : 0;
  let days = req ? clampInt(req.days, 1, 7, 1) : 1;
  const normalizedKind = kind === 'current' ? 'current' : 'daily';
  if (normalizedKind === 'current') days = 1;

  return {
    intent: intent === 'weather' ? 'weather' : 'other',
    location_query: locationQuery && locationQuery.length <= 80 ? locationQuery : null,
    use_last_location: useLast,
    request: { kind: normalizedKind, start_offset_days: startOffsetDays, days },
    temperature_unit: normalizeTemperatureUnit(parsed && parsed.temperature_unit),
    wind_speed_unit: normalizeWindUnit(parsed && parsed.wind_speed_unit),
  };
}

async function geocode(place, turnId) {
  const span = telemetry.startSpan('weather.geocode', { turn_id: turnId, query: place });
  let status = 'success';
  try {
    const url =
      'https://geocoding-api.open-meteo.com/v1/search?name=' +
      encodeURIComponent(place) +
      '&count=5&language=en&format=json';
    const responseText = await fetch(url, { method: 'GET' });
    const data = JSON.parse(responseText || '{}');
    const results = Array.isArray(data.results) ? data.results : [];
    if (!results.length) return null;
    const best = results[0];
    return {
      name: best.name || place,
      admin1: best.admin1 || '',
      country: best.country || '',
      latitude: best.latitude,
      longitude: best.longitude,
    };
  } catch (e) {
    status = 'error';
    if (isFetchBlockedError(e)) return { __error: allowlistHint('Open-Meteo geocoding') };
    return { __error: 'Sorry, I could not reach the geocoding service.' };
  } finally {
    telemetry.endSpan(span, { turn_id: turnId, status });
  }
}

async function fetchForecast(resolved, units, forecastDays, turnId) {
  const span = telemetry.startSpan('weather.forecast', {
    turn_id: turnId,
    latitude: resolved.latitude,
    longitude: resolved.longitude,
    temperature_unit: units.temperature_unit,
    wind_speed_unit: units.wind_speed_unit,
    forecast_days: forecastDays,
  });
  let status = 'success';
  try {
    const requestedDays = Math.min(7, Math.max(1, Number(forecastDays) || 3));
    const url =
      'https://api.open-meteo.com/v1/forecast' +
      '?latitude=' +
      encodeURIComponent(resolved.latitude) +
      '&longitude=' +
      encodeURIComponent(resolved.longitude) +
      '&current_weather=true' +
      '&daily=weathercode,temperature_2m_max,temperature_2m_min,precipitation_probability_max' +
      '&forecast_days=' +
      encodeURIComponent(String(requestedDays)) +
      '&timezone=auto' +
      '&temperature_unit=' +
      encodeURIComponent(units.temperature_unit) +
      '&wind_speed_unit=' +
      encodeURIComponent(units.wind_speed_unit);
    const responseText = await fetch(url, { method: 'GET' });
    return JSON.parse(responseText || '{}');
  } catch (e) {
    status = 'error';
    if (isFetchBlockedError(e)) return { __error: allowlistHint('Open-Meteo forecast') };
    return { __error: 'Sorry, I could not reach the weather service.' };
  } finally {
    telemetry.endSpan(span, { turn_id: turnId, status });
  }
}

function formatCurrentResponse(resolved, forecast, units) {
  const loc = locationLabel(resolved);
  const cw = forecast.current_weather || {};
  const temp = spokenDegrees(cw.temperature, units.temperature_unit) || 'an unknown temperature';
  const wind = spokenWind(cw.windspeed, units.wind_speed_unit);
  const desc = describeWeatherCode(cw.weathercode);
  if (wind) return `${loc}: ${desc}, around ${temp}, with wind at ${wind}.`;
  return `${loc}: ${desc}, around ${temp}.`;
}

function formatDailyResponse(resolved, forecast, request, units) {
  const loc = locationLabel(resolved);
  const daily = forecast.daily || {};

  const times = Array.isArray(daily.time) ? daily.time : [];
  const tMaxArr = Array.isArray(daily.temperature_2m_max) ? daily.temperature_2m_max : [];
  const tMinArr = Array.isArray(daily.temperature_2m_min) ? daily.temperature_2m_min : [];
  const popArr = Array.isArray(daily.precipitation_probability_max) ? daily.precipitation_probability_max : [];
  const codeArr = Array.isArray(daily.weathercode) ? daily.weathercode : [];

  const startIdx = request.start_offset_days;
  const days = request.days;

  const maxIdx = Math.min(startIdx + days, times.length, tMaxArr.length, tMinArr.length);
  if (maxIdx <= startIdx) return `Sorry, I couldn't read the forecast data for ${loc}.`;

  if (days === 1) {
    const dayLabel = times[startIdx] ? String(times[startIdx]) : startIdx === 1 ? 'tomorrow' : 'today';
    const desc = codeArr[startIdx] === undefined ? '' : describeWeatherCode(codeArr[startIdx]);
    const pop = popArr[startIdx];
    const parts = [];
    parts.push(`${dayLabel} in ${loc}:`);
    if (desc) parts.push(desc + '.');
    const hi = spokenDegrees(tMaxArr[startIdx], units.temperature_unit);
    const lo = spokenDegrees(tMinArr[startIdx], units.temperature_unit);
    if (hi && lo) parts.push(`High around ${hi}, low around ${lo}.`);
    const popSpoken = spokenPercent(pop);
    if (popSpoken) parts.push(`Max precipitation chance ${popSpoken}.`);
    return parts.join(' ');
  }

  const lines = [];
  lines.push(`Next ${days} days in ${loc}:`);
  for (let i = startIdx; i < maxIdx; i++) {
    const dayLabel = times[i] ? String(times[i]) : `day ${i - startIdx + 1}`;
    const desc = codeArr[i] === undefined ? '' : describeWeatherCode(codeArr[i]);
    const pop = popArr[i];
    const lineParts = [];
    lineParts.push(`${dayLabel}:`);
    if (desc) lineParts.push(desc + ',');
    const hi = spokenDegrees(tMaxArr[i], units.temperature_unit);
    const lo = spokenDegrees(tMinArr[i], units.temperature_unit);
    if (hi && lo) lineParts.push(`high ${hi}, low ${lo}`);
    const popSpoken = spokenPercent(pop);
    if (popSpoken) lineParts.push(`(${popSpoken} chance)`);
    lines.push(lineParts.join(' '));
  }
  return lines.join(' ');
}

async function process(packet) {
  if (packet.type !== 'Transcription') return null;

  let text = '';
  if (packet.data && packet.data.text) text = packet.data.text;
  else if (packet.text) text = packet.text;
  text = normalizeWhitespace(text);
  if (!text) return null;

  const turnId = `turn-${Date.now()}-${++turnCounter}`;

  const parsedRaw = await parseRequestWithOpenAI(text.slice(0, 500), turnId);
  if (parsedRaw && parsedRaw.__error) return { type: 'Text', data: String(parsedRaw.__error) };

  const parsed = normalizeParsedRequest(parsedRaw);
  if (parsed.intent !== 'weather') {
    return { type: 'Text', data: "Ask me about the weather, like: what's the weather in Seattle today?" };
  }

  const request = parsed.request;
  const units = { temperature_unit: parsed.temperature_unit, wind_speed_unit: parsed.wind_speed_unit };

  let resolved = null;
  if (parsed.location_query) {
    resolved = await geocode(parsed.location_query, turnId);
    if (resolved && resolved.__error) return { type: 'Text', data: String(resolved.__error) };
    if (!resolved || resolved.latitude === undefined || resolved.longitude === undefined) {
      return { type: 'Text', data: `Sorry, I couldn't find a location named ${parsed.location_query}.` };
    }
    lastResolved = resolved;
  } else if (parsed.use_last_location && lastResolved) {
    resolved = lastResolved;
  } else if (lastResolved) {
    resolved = lastResolved;
  } else {
    return { type: 'Text', data: 'Which city should I check the weather for?' };
  }

  const forecastDays = Math.min(7, Math.max(1, request.start_offset_days + request.days));
  const forecast = await fetchForecast(resolved, units, forecastDays, turnId);
  if (forecast && forecast.__error) return { type: 'Text', data: String(forecast.__error) };

  const llmReply = await speakResponseWithOpenAI(resolved, parsed, forecast, text, turnId);
  const response = llmReply
    ? llmReply
    : request.kind === 'current'
      ? formatCurrentResponse(resolved, forecast, units)
      : formatDailyResponse(resolved, forecast, request, units);

  pushConversation('user', text);
  pushConversation('assistant', response);
  return { type: 'Text', data: response };
}
