//
// SpelbookMain.cpp — minimal standalone test for SpelbookBackend.
//
// Build:  cmake --build build --target SpelbookApp
// Run:    LD_LIBRARY_PATH=../target/release ./build/SpelbookApp
//
// Performs a cache search for all programs (empty query) and prints the JSON
// result to stdout.  No Qt event loop is required because SpelbookBackend
// calls are synchronous.

#include "SpelbookBackend.h"

#include <QCoreApplication>
#include <QJsonDocument>
#include <QTextStream>

int main(int argc, char* argv[])
{
    QCoreApplication app(argc, argv);

    SpelbookBackend backend;

    QTextStream out(stdout);

    // 1. Search all cached programs (empty query = no filter).
    out << "=== searchPrograms(\"\") ===" << Qt::endl;
    const QString searchResult = backend.searchPrograms(QStringLiteral(""));
    // Pretty-print the JSON if possible.
    const QJsonDocument doc =
        QJsonDocument::fromJson(searchResult.toUtf8());
    if (!doc.isNull()) {
        out << doc.toJson(QJsonDocument::Indented) << Qt::endl;
    } else {
        out << searchResult << Qt::endl;
    }

    // 2. Example: search for programs matching "token".
    out << "=== searchPrograms(\"token\") ===" << Qt::endl;
    const QString tokenResult = backend.searchPrograms(QStringLiteral("token"));
    const QJsonDocument doc2 =
        QJsonDocument::fromJson(tokenResult.toUtf8());
    if (!doc2.isNull()) {
        out << doc2.toJson(QJsonDocument::Indented) << Qt::endl;
    } else {
        out << tokenResult << Qt::endl;
    }

    return 0;
}
