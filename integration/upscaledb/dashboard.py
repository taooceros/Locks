#!/usr/bin/env python3
"""Read-only local integration dashboard; stdlib only, no benchmark execution.

GET / serves the companion HTML; GET /api/progress returns a bounded snapshot.
Only discovered case analysis PNGs/summary.json and four fixed overview files
are downloadable. Responses are no-store. This is a loopback tool, not a public
service. Parent must launch with CPU affinity outside benchmark worker CPUs.
"""
import argparse
from collections import OrderedDict
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import re
import stat
import threading
import time
from urllib.parse import quote, unquote, urlsplit

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
VARIANTS = ('native', 'refactored', 'bridge_mutex', 'fc', 'fc_pq',
            'profile', 'bridge_mutex_profile', 'fc_profile', 'fc_pq_profile')
NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,179}\Z')
TRIAL = re.compile(r'block-(\d{4,})\.([a-z_]+)\.json\Z')
JSON_LIMIT = 4 * 1024 * 1024
FILE_LIMIT = 16 * 1024 * 1024
OVERVIEW_FILES = {
    'fixed_elapsed_scaling.png': 'image/png',
    'fixed_fc_pq_speedup.png': 'image/png',
    'overview.csv': 'text/csv; charset=utf-8',
    'overview.json': 'application/json; charset=utf-8',
}


def now():
    return datetime.now(timezone.utc).isoformat(timespec='seconds')


def text(value, limit=1200):
    return value[:limit] if isinstance(value, str) else ''


def object_value(value):
    return value if isinstance(value, dict) else {}


def finite_constant(value):
    raise ValueError('nonfinite JSON')


def finite_float(value):
    result = float(value)
    if not math.isfinite(result):
        raise ValueError('nonfinite JSON number')
    return result


def png_header(data):
    return (len(data) >= 24 and data[:16] == b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR'
            and 0 < int.from_bytes(data[16:20], 'big') <= 16000
            and 0 < int.from_bytes(data[20:24], 'big') <= 16000)


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON key')
        result[key] = value
    return result


def profile(variant):
    return variant == 'profile' or variant.endswith('_profile')


class Artifacts:
    def __init__(self, root):
        self.root = root.resolve()
        self.cache = OrderedDict()
        self.cache_bytes = 0
        self.lock = threading.Lock()
        self.snapshot = None
        self.snapshot_at = 0
        self.allowed = set()

    def display(self, relative=''):
        path = self.root / relative
        try:
            return str(path.relative_to(ROOT))
        except ValueError:
            return 'artifact-root/' + relative

    def open(self, relative, directory=False):
        """Walk with directory FDs; reject traversal and symlinks at every hop."""
        parts = relative.split('/') if relative else []
        if any(not NAME.fullmatch(p) or p in ('.', '..') for p in parts):
            raise ValueError('unsafe artifact path')
        fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for index, part in enumerate(parts):
                flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
                if index < len(parts) - 1 or directory:
                    flags |= os.O_DIRECTORY
                child = os.open(part, flags, dir_fd=fd)
                os.close(fd)
                fd = child
            return fd
        except BaseException:
            os.close(fd)
            raise

    def entries(self, relative=''):
        try:
            fd = self.open(relative, directory=True)
            try:
                with os.scandir(fd) as entries:
                    result = []
                    for index, entry in enumerate(entries):
                        if index >= 1024:
                            return sorted(result), 'directory listing capped at 1024 entries'
                        if NAME.fullmatch(entry.name):
                            result.append((entry.name, entry.is_dir(follow_symlinks=False)))
                    return sorted(result), None
            finally:
                os.close(fd)
        except (OSError, ValueError):
            return [], 'directory unavailable'

    def load(self, relative, trial=False):
        """Cache by inode/size/mtime; retain compact trial state, not raw stdout."""
        fd = None
        try:
            fd = self.open(relative)
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_size > JSON_LIMIT:
                return None, 'not a regular JSON file or exceeds 4 MiB'
            stamp = (info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
            key = (relative, trial)
            old = self.cache.get(key)
            if old and old[0] == stamp:
                self.cache.move_to_end(key)
                return old[1], old[2]
            with os.fdopen(fd, 'rb') as stream:
                fd = None
                raw = stream.read(JSON_LIMIT + 1)
                end = os.fstat(stream.fileno())
            if len(raw) > JSON_LIMIT or (end.st_size, end.st_mtime_ns) != (info.st_size, info.st_mtime_ns):
                return None, 'record changing; waiting for next refresh'
            value = json.loads(raw, parse_constant=finite_constant, parse_float=finite_float,
                               object_pairs_hook=no_duplicate_keys)
            if not isinstance(value, dict):
                raise ValueError('JSON object required')
            if trial:
                success = value.get('success')
                credible = (value.get('schema') == 1 and type(success) is bool
                            and type(value.get('block')) is int and value.get('variant') in VARIANTS)
                if success is True:
                    credible = credible and (type(value.get('returncode')) is int
                        and value['returncode'] == 0 and value.get('timed_out') is False
                        and object_value(value.get('result')).get('status') == 'ok'
                        and all(k in value and value[k] is None for k in
                                ('interrupted_signal', 'artifact_error', 'launch_error', 'cleanup_error')))
                value = {'schema': value.get('schema'), 'block': value.get('block'),
                         'variant': value.get('variant'),
                         'recorded_at': datetime.fromtimestamp(info.st_mtime, timezone.utc).isoformat(timespec='seconds'),
                         'state': ('completed' if success else 'failed') if credible else 'unreadable'}
            weight = 512 if trial else len(raw)
            if old:
                self.cache_bytes -= old[3]
            self.cache[key] = (stamp, value, None, weight)
            self.cache_bytes += weight
            while len(self.cache) > 24000 or self.cache_bytes > 24 * 1024 * 1024:
                _, removed = self.cache.popitem(last=False)
                self.cache_bytes -= removed[3]
            return value, None
        except FileNotFoundError:
            return None, 'missing'
        except (OSError, ValueError, UnicodeError, RecursionError, OverflowError):
            return None, 'unreadable or incomplete JSON'
        finally:
            if fd is not None:
                os.close(fd)

    def case(self, relative, warnings, allowed, budget):
        manifest, error = self.load(relative + '/manifest.json')
        if error == 'missing':
            return None
        item = {'name': relative, 'path': self.display(relative), 'error': error,
                'kind': 'unknown', 'lanes': [], 'plots': [], 'analysis': None, 'latest_trial_record_at': ''}
        cfg = object_value((manifest or {}).get('config'))
        variants = cfg.get('variants')
        files = (manifest or {}).get('trial_files')
        repetitions = cfg.get('repetitions')
        if (error or manifest.get('schema') != 1 or not isinstance(variants, list)
                or not variants or any(not isinstance(v, str) or v not in VARIANTS for v in variants)
                or len(set(variants)) != len(variants) or type(repetitions) is not int
                or not 1 <= repetitions <= 20000 or not isinstance(files, list)
                or len(files) > 20000 or len(files) != repetitions * len(variants)):
            item['error'] = error or 'invalid or oversized runner manifest; counts unavailable'
            return item
        expected = {f'block-{b:04d}.{v}.json' for b in range(repetitions) for v in variants}
        if any(not isinstance(f, str) for f in files) or set(files) != expected:
            item['error'] = 'trial_files does not match the configured trial matrix'
            return item
        if len(files) > budget[0]:
            item['error'] = 'Snapshot trial budget (20,000 records) reached; counts unavailable'
            return item
        budget[0] -= len(files)
        item['kind'] = ('smoke — not primary' if cfg.get('smoke') is True else
                        'primary candidate' if cfg.get('smoke') is False else 'unclassified')
        item['config'] = {k: cfg.get(k) for k in ('mode', 'workers', 'cpus', 'layout', 'repetitions')}
        counts = {v: dict(variant=v, kind='profile' if profile(v) else 'primary',
                         completed=0, failed=0, pending=0, unreadable=0, planned=repetitions)
                  for v in variants}
        for filename in files:
            match = TRIAL.fullmatch(filename)
            block, variant = int(match[1]), match[2]
            record, error = self.load(relative + '/' + filename, trial=True)
            state = 'pending' if error == 'missing' else 'unreadable'
            if record and record.get('block') == block and record.get('variant') == variant:
                state = record['state']
                if state in ('completed', 'failed'):
                    item['latest_trial_record_at'] = max(item['latest_trial_record_at'], record['recorded_at'])
            counts[variant][state] += 1
        item['lanes'] = list(counts.values())
        summary_path = relative + '/analysis/summary.json'
        summary, error = self.load(summary_path)
        if summary and summary.get('schema') == 1:
            groups = object_value(summary.get('groups'))
            item['analysis'] = {'classification': text(summary.get('classification')),
                'failure_count': summary.get('failure_count') if type(summary.get('failure_count')) is int else None,
                'groups': {v: {k: g.get(k) for k in ('kind', 'n_success', 'n_expected', 'n_failed_or_missing')}
                           for v, g in groups.items() if v in VARIANTS and isinstance(g, dict)},
                'paired_comparison_count': len(object_value(summary.get('paired_comparisons'))),
                'url': '/artifacts/' + quote(summary_path, safe='/')}
            allowed.add(summary_path)
        elif error != 'missing':
            warnings.append(self.display(summary_path) + ': ' + (error or 'unsupported summary schema'))
        entries, listing_error = self.entries(relative + '/analysis')
        if listing_error and listing_error != 'directory unavailable':
            warnings.append(relative + '/analysis: ' + listing_error)
        for name, directory in entries:
            if not directory and name.endswith('.png'):
                if len(item['plots']) >= 48:
                    warnings.append(relative + ': plot list capped at 48')
                    break
                path = relative + '/analysis/' + name
                try:
                    fd = self.open(path)
                    with os.fdopen(fd, 'rb') as stream:
                        info = os.fstat(stream.fileno())
                        if not stat.S_ISREG(info.st_mode) or info.st_size > FILE_LIMIT or not png_header(stream.read(24)):
                            raise ValueError('invalid PNG')
                except (OSError, ValueError):
                    warnings.append(self.display(path) + ': PNG unavailable or incomplete')
                    continue
                allowed.add(path)
                item['plots'].append({'name': name, 'kind': 'profile' if name.startswith('profile_') else 'primary',
                                      'url': '/artifacts/' + quote(path, safe='/') +
                                      f'?v={info.st_mtime_ns}-{info.st_size}'})
        return item

    def overview(self):
        files = []
        for name, mime in OVERVIEW_FILES.items():
            relative = 'overview/' + name
            item = {'name': name, 'mime': mime, 'state': 'pending', 'url': None}
            try:
                fd = self.open(relative)
                with os.fdopen(fd, 'rb') as stream:
                    info = os.fstat(stream.fileno())
                    if not stat.S_ISREG(info.st_mode) or info.st_size > FILE_LIMIT:
                        raise ValueError('file unavailable')
                    if mime == 'image/png' and not png_header(stream.read(24)):
                        raise ValueError('invalid PNG')
                if name == 'overview.json':
                    metadata, error = self.load(relative)
                    if error or not metadata or metadata.get('schema') != 1:
                        raise ValueError('invalid overview JSON')
                item.update(state='available', url='/overview/' + name +
                            f'?v={info.st_mtime_ns}-{info.st_size}')
            except FileNotFoundError:
                pass
            except (OSError, ValueError):
                item['state'] = 'unreadable'
            files.append(item)
        return {'files': files}

    def progress(self):
        with self.lock:
            if self.snapshot is not None and time.monotonic() - self.snapshot_at < 1:
                return self.snapshot
            warnings, allowed = [], set()
            live, error = self.load('live-progress.json')
            if error or (live or {}).get('schema') != 1:
                warnings.append('Main progress: ' + (error or 'unsupported schema'))
                live = {}
            stages = live.get('stages', [])
            checks = live.get('checks', [])
            events = live.get('events', [])
            events = sorted((v for v in events if isinstance(v, dict)),
                            key=lambda v: text(v.get('time')), reverse=True) if isinstance(events, list) else []
            def rows(values, keys, limit):
                return [{k: text(v.get(k)) for k in keys} for v in values[:limit]
                        if isinstance(v, dict)] if isinstance(values, list) else []
            builds = []
            for variant in VARIANTS:
                relative = 'build-' + variant + '.json'
                build, error = self.load(relative)
                valid = bool(build and build.get('variant') == variant and build.get('schema') in (1, 2)
                             and isinstance(build.get('binary'), str)
                             and re.fullmatch(r'[0-9a-f]{64}', text(object_value(build.get('hashes')).get('binary_sha256'))))
                binary = text((build or {}).get('binary'))
                if binary:
                    try:
                        binary = str(Path(binary).relative_to(ROOT))
                    except ValueError:
                        binary = Path(binary).name
                builds.append({'variant': variant, 'kind': 'profile' if profile(variant) else 'primary',
                               'state': 'recorded' if valid else 'pending' if error == 'missing' else 'unreadable',
                               'path': self.display(relative), 'binary': binary,
                               'schema': (build or {}).get('schema'),
                               'sha256': text(object_value((build or {}).get('hashes')).get('binary_sha256'), 64) if valid else ''})
            cases = []
            budget = [20000]
            entries, error = self.entries()
            if error:
                warnings.append('Artifact root: ' + error)
            candidates = [n for n, d in entries if d and n not in ('results', 'overview') and not
                          (n.startswith(('src-', 'build-', 'target')) or n in ('build', 'source', 'node_modules'))]
            result_entries, error = self.entries('results')
            if error and error != 'directory unavailable':
                warnings.append('results: ' + error)
            candidates += ['results/' + n for n, d in result_entries if d]
            # Only probe manifest.json in immediate candidate directories. Never recurse.
            for relative in candidates[:128]:
                case = self.case(relative, warnings, allowed, budget)
                if case:
                    cases.append(case)
            if len(candidates) > 128:
                warnings.append('Case discovery capped at 128 directories')
            self.allowed = allowed
            self.snapshot = {'schema': 1, 'refreshed_at': now(), 'artifact_root': self.display(),
                'updated_at': text(live.get('updated_at')), 'headline': text(live.get('headline')),
                'latest_trial_record_at': max((c['latest_trial_record_at'] for c in cases), default=''),
                'stages': rows(stages, ('id', 'title', 'status', 'detail'), 32),
                'checks': rows(checks, ('name', 'status', 'detail'), 64),
                'events': rows(events, ('time', 'message', 'kind'), 50),
                'overview': self.overview(),
                'builds': builds, 'cases': cases, 'warnings': warnings[:64]}
            self.snapshot_at = time.monotonic()
            return self.snapshot

    def download(self, relative, overview=False):
        if overview:
            if relative not in {'overview/' + name for name in OVERVIEW_FILES}:
                raise FileNotFoundError
        else:
            self.progress()
            with self.lock:
                if relative not in self.allowed:
                    raise FileNotFoundError
        try:
            fd = self.open(relative)
        except OSError:
            if overview:
                raise FileNotFoundError from None
            raise
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_size > FILE_LIMIT:
                raise ValueError('file unavailable')
            data = stream.read(FILE_LIMIT + 1)
        if len(data) > FILE_LIMIT:
            raise ValueError('file too large')
        if overview and relative == 'overview/overview.csv':
            data.decode('utf-8')
            return data, OVERVIEW_FILES['overview.csv']
        if relative.endswith('.png'):
            if not png_header(data):
                raise ValueError('invalid PNG')
            return data, 'image/png'
        parsed = json.loads(data, parse_constant=finite_constant, parse_float=finite_float,
                            object_pairs_hook=no_duplicate_keys)
        if not isinstance(parsed, dict) or parsed.get('schema') != 1:
            raise ValueError('invalid summary')
        return data, 'application/json; charset=utf-8'


class Server(ThreadingHTTPServer):
    daemon_threads = True


class Handler(BaseHTTPRequestHandler):
    server_version = 'IntegrationDashboard/1'

    def reply(self, status, data, content_type):
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Referrer-Policy', 'no-referrer')
        self.send_header('Content-Security-Policy', "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'")
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(data)

    def error(self, status, code):
        self.reply(status, json.dumps({'error': {'code': code}}).encode(), 'application/json; charset=utf-8')

    def do_GET(self):
        try:
            path = unquote(urlsplit(self.path).path, errors='strict')
            if path == '/':
                self.reply(200, self.server.page, 'text/html; charset=utf-8')
            elif path == '/api/progress':
                data = json.dumps(self.server.artifacts.progress(), allow_nan=False).encode()
                self.reply(200, data, 'application/json; charset=utf-8')
            elif path.startswith('/artifacts/'):
                data, mime = self.server.artifacts.download(path[len('/artifacts/'):])
                self.reply(200, data, mime)
            elif path.startswith('/overview/'):
                data, mime = self.server.artifacts.download(path[1:], overview=True)
                self.reply(200, data, mime)
            else:
                self.error(404, 'not_found')
        except (BrokenPipeError, ConnectionResetError):
            pass
        except FileNotFoundError:
            self.error(404, 'not_found')
        except (OSError, ValueError, UnicodeError, RecursionError, OverflowError):
            self.error(503, 'artifact_unavailable')

    do_HEAD = do_GET

    def do_POST(self):
        self.error(405, 'read_only')

    do_PUT = do_POST
    do_DELETE = do_POST
    do_PATCH = do_POST

    def log_message(self, format, *args):
        # Polling should not generate continual terminal I/O during benchmarks.
        if len(args) > 1 and str(args[1]) not in ('200', '404'):
            super().log_message(format, *args)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8765)
    parser.add_argument('--artifact-root', type=Path, default=ROOT / '.worktree' / 'upscaledb')
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error('--port must be between 0 and 65535')
    page = (HERE / 'dashboard.html').read_bytes()
    with Server((args.host, args.port), Handler) as server:
        server.page = page
        server.artifacts = Artifacts(args.artifact_root.expanduser())
        host = args.host if args.host not in ('0.0.0.0', '') else '127.0.0.1'
        print(f'Dashboard ready: http://{host}:{server.server_port}/', flush=True)
        print('Read-only; pin this process outside benchmark CPUs. Non-loopback binding is unauthenticated.', flush=True)
        try:
            server.serve_forever(poll_interval=0.5)
        except KeyboardInterrupt:
            pass


if __name__ == '__main__':
    main()
