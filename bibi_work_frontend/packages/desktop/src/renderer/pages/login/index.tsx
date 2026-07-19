const loginLogo = './biwork-logo.svg';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { changeLanguage } from '@/renderer/services/i18n';
import { useNavigate } from 'react-router-dom';
import AppLoader from '@renderer/components/layout/AppLoader';
import { useAuth } from '../../hooks/context/AuthContext';
import './LoginPage.css';

type MessageState = {
  type: 'error' | 'success';
  text: string;
};

const LoginPage: React.FC = () => {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const { status, login } = useAuth();

  const [message, setMessage] = useState<MessageState | null>(null);
  const [loading, setLoading] = useState(false);

  const messageTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    document.body.classList.add('login-page-active');
    return () => {
      document.body.classList.remove('login-page-active');
      if (messageTimer.current) {
        window.clearTimeout(messageTimer.current);
      }
    };
  }, []);

  useEffect(() => {
    document.title = t('login.pageTitle');
  }, [t]);

  useEffect(() => {
    document.documentElement.lang = i18n.language;
  }, [i18n.language]);

  useEffect(() => {
    if (status === 'authenticated') {
      void navigate('/guid', { replace: true });
    }
  }, [navigate, status]);

  const clearMessageLater = useCallback(() => {
    if (messageTimer.current) {
      window.clearTimeout(messageTimer.current);
    }
    messageTimer.current = window.setTimeout(() => {
      setMessage((prev) => (prev?.type === 'success' ? prev : null));
    }, 5000);
  }, []);

  const showMessage = useCallback(
    (next: MessageState) => {
      setMessage(next);
      if (next.type === 'error') {
        clearMessageLater();
      }
    },
    [clearMessageLater]
  );

  const supportedLanguages = useMemo<{ code: string; label: string }[]>(
    () => [
      { code: 'zh-CN', label: '简体中文' },
      { code: 'zh-TW', label: '繁體中文' },
      { code: 'ja-JP', label: '日本語' },
      { code: 'ko-KR', label: '한국어' },
      { code: 'tr-TR', label: 'Türkçe' },
      { code: 'uk-UA', label: 'Українська' },
      { code: 'pt-BR', label: 'Português (BR)' },
      { code: 'de-DE', label: 'Deutsch' },
      { code: 'es-ES', label: 'Español' },
      { code: 'fa-IR', label: 'فارسی' },
      { code: 'en-US', label: 'English' },
    ],
    []
  );

  const handleLanguageChange = useCallback((event: React.ChangeEvent<HTMLSelectElement>) => {
    const nextLanguage = event.target.value;
    changeLanguage(nextLanguage).catch((error: Error) => {
      console.error('Failed to change language:', error);
    });
  }, []);

  const handleSubmit = useCallback(
    async (event: React.FormEvent) => {
      event.preventDefault();

      setLoading(true);
      setMessage(null);

      const result = await login();

      if (result.success || result.redirected) {
        showMessage({
          type: 'success',
          text: result.message ?? t('login.oidcRedirecting', { defaultValue: 'Redirecting to FerrisKey...' }),
        });
      } else {
        const errorText = (() => {
          switch (result.code) {
            case 'networkError':
              return t('login.errors.networkError');
            case 'redirecting':
              return result.message ?? t('login.oidcRedirecting', { defaultValue: 'Redirecting to FerrisKey...' });
            default:
              return result.message ?? t('login.errors.unknown');
          }
        })();

        showMessage({ type: 'error', text: errorText });
      }

      setLoading(false);
    },
    [login, showMessage, t]
  );

  if (status === 'checking') {
    return <AppLoader />;
  }

  return (
    <div className='login-page'>
      {/* <div className='login-page__background' aria-hidden='true'>
        <div className='login-page__background-circle login-page__background-circle--lg' />
        <div className='login-page__background-circle login-page__background-circle--md' />
        <div className='login-page__background-circle login-page__background-circle--sm' />
      </div> */}

      <div className='login-page__card'>
        <label className='login-page__lang-select-wrapper' htmlFor='lang-select'>
          <select
            id='lang-select'
            className='login-page__lang-select'
            value={i18n.language}
            onChange={handleLanguageChange}
          >
            {supportedLanguages.map((lang) => (
              <option key={lang.code} value={lang.code}>
                {lang.label}
              </option>
            ))}
          </select>
        </label>

        <div className='login-page__header'>
          <div className='login-page__logo'>
            <img src={loginLogo} alt={t('login.brand')} />
          </div>
          <h1 className='login-page__title'>{t('login.brand')}</h1>
          <p className='login-page__subtitle'>{t('login.subtitle')}</p>
        </div>

        <form className='login-page__form' onSubmit={handleSubmit}>
          <button type='submit' className='login-page__submit' disabled={loading}>
            {loading && (
              <svg className='login-page__spinner' viewBox='0 0 24 24' width='18' height='18'>
                <circle
                  cx='12'
                  cy='12'
                  r='10'
                  stroke='currentColor'
                  strokeWidth='3'
                  fill='none'
                  strokeDasharray='50'
                  strokeDashoffset='25'
                  strokeLinecap='round'
                />
              </svg>
            )}
            <span>
              {loading
                ? t('login.oidcSubmitting', { defaultValue: 'Opening FerrisKey...' })
                : t('login.oidcSubmit', { defaultValue: 'Continue with FerrisKey' })}
            </span>
          </button>

          <div
            role='alert'
            aria-live='polite'
            className={`login-page__message ${message ? 'login-page__message--visible' : ''} ${message ? (message.type === 'success' ? 'login-page__message--success' : 'login-page__message--error') : ''}`}
            hidden={!message}
          >
            {message?.text}
          </div>
        </form>

        <div className='login-page__footer'>
          <div className='login-page__footer-content'>
            <span>{t('login.footerPrimary')}</span>
            <span className='login-page__footer-divider'>•</span>
            <span>{t('login.footerSecondary')}</span>
          </div>
        </div>
      </div>
    </div>
  );
};

export default LoginPage;
