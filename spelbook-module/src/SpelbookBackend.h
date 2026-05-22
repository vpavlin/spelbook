#pragma once

#include <QObject>
#include <QString>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>

// ── Rust FFI declarations ─────────────────────────────────────────────────────

extern "C" {
    char* lez_storage_fetch_idl(const char* args_json);
    char* lez_registry_search(const char* args_json);
    char* lez_registry_get_by_id(const char* args_json);
    char* lez_registry_register(const char* args_json);
    void  lez_registry_free_string(char* s);
}

// ── SpelbookBackend ───────────────────────────────────────────────────────────

/**
 * Qt backend that wraps the LEZ registry C FFI.
 *
 * All Q_INVOKABLE methods accept and return JSON strings so that QML callers
 * can use JSON.parse() / JSON.stringify() directly.
 *
 * Memory contract: every string returned by the FFI is freed internally;
 * callers of these methods receive a plain Qt QString copy.
 */
class SpelbookBackend : public QObject
{
    Q_OBJECT

public:
    explicit SpelbookBackend(QObject* parent = nullptr) : QObject(parent) {}

    // ── Storage ──────────────────────────────────────────────────────────────

    /**
     * Fetch and parse an IDL JSON from Logos Storage (Codex).
     *
     * argsJson: {"logos_storage_url":"http://...","cid":"bafy..."}
     * Returns:  {"success":true,"idl":{...}}  or  {"success":false,"error":"..."}
     */
    Q_INVOKABLE QString fetchIdl(const QString& storageUrl, const QString& cid)
    {
        QJsonObject args;
        args["logos_storage_url"] = storageUrl;
        args["cid"] = cid;
        return callFfi(args, lez_storage_fetch_idl);
    }

    // ── Registry / Cache ─────────────────────────────────────────────────────

    /**
     * Search cached program entries by name, description, or tags.
     *
     * query: substring to search for; pass "" to return all cached entries.
     * Returns: {"success":true,"programs":[...],"count":N}
     */
    Q_INVOKABLE QString searchPrograms(const QString& query)
    {
        QJsonObject args;
        args["query"] = query;
        return callFfi(args, lez_registry_search);
    }

    /**
     * Fetch a single program entry by program_id from the on-chain registry.
     * The result is also written to the local cache.
     *
     * Returns: {"success":true,"entry":{...},"entry_pda":"..."}
     */
    Q_INVOKABLE QString getProgram(const QString& programIdHex,
                                   const QString& sequencerUrl,
                                   const QString& registryProgramIdHex,
                                   const QString& walletPath)
    {
        QJsonObject args;
        args["program_id"]          = programIdHex;
        args["sequencer_url"]       = sequencerUrl;
        args["registry_program_id"] = registryProgramIdHex;
        args["wallet_path"]         = walletPath;
        return callFfi(args, lez_registry_get_by_id);
    }

    /**
     * Register a new program in the on-chain LEZ registry.
     * argsJson must be a complete JSON object (all required fields).
     *
     * Returns: {"success":true,"tx_hash":"...","entry_pda":"..."}
     */
    Q_INVOKABLE QString registerProgram(const QString& argsJson)
    {
        // The caller provides the full args JSON; forward it verbatim.
        return callFfiRaw(argsJson, lez_registry_register);
    }

private:
    // ── Helpers ──────────────────────────────────────────────────────────────

    /** Build args JSON from a QJsonObject, call FFI fn, return result string. */
    static QString callFfi(const QJsonObject& args, char* (*fn)(const char*))
    {
        const QByteArray argsBytes =
            QJsonDocument(args).toJson(QJsonDocument::Compact);
        return callFfiRaw(QString::fromUtf8(argsBytes), fn);
    }

    /** Call FFI fn with a raw JSON string, free the C string, return a QString. */
    static QString callFfiRaw(const QString& argsJson, char* (*fn)(const char*))
    {
        const QByteArray utf8 = argsJson.toUtf8();
        char* raw = fn(utf8.constData());
        if (!raw) {
            return QStringLiteral(R"({"success":false,"error":"FFI returned null"})");
        }
        QString result = QString::fromUtf8(raw);
        lez_registry_free_string(raw);
        return result;
    }
};
