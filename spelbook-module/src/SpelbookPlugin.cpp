#include "SpelbookBackend.h"

#include <QLabel>
#include <QObject>
#include <QQmlContext>
#include <QQuickWidget>
#include <QString>
#include <QWidget>

/**
 * SpelbookPlugin — minimal Logos module plugin.
 *
 * Creates a SpelbookBackend instance and exposes it to QML as the context
 * property "spelbook".  The widget returned by createWidget() is a simple
 * status label; swap it for a QQuickWidget + QML source file for a richer UI.
 */
class SpelbookPlugin : public QObject
{
    Q_OBJECT

public:
    explicit SpelbookPlugin(QObject* parent = nullptr)
        : QObject(parent), m_backend(nullptr)
    {}

    /**
     * Create and return the module widget.
     *
     * api: optional Logos module-API object (unused in this minimal impl).
     *
     * The returned widget exposes "spelbook" in its QML engine context so
     * that any QML loaded into the view can call spelbook.searchPrograms(),
     * spelbook.getProgram(), etc.
     */
    QWidget* createWidget(QObject* api = nullptr)
    {
        Q_UNUSED(api)

        if (!m_backend)
            m_backend = new SpelbookBackend(this);

        // Minimal status label — replace with a QQuickWidget + QML file for a
        // real UI.  Example:
        //
        //   auto* view = new QQuickWidget();
        //   view->engine()->rootContext()->setContextProperty("spelbook", m_backend);
        //   view->setSource(QUrl("qrc:/spelbook/Main.qml"));
        //   view->setResizeMode(QQuickWidget::SizeRootObjectToView);
        //   return view;

        auto* label = new QLabel(QStringLiteral("Spelbook — LEZ Program Registry"));
        label->setAlignment(Qt::AlignCenter);
        return label;
    }

    /** Direct access to the backend (e.g. for headless / test use). */
    SpelbookBackend* backend() const { return m_backend; }

private:
    SpelbookBackend* m_backend;
};

// Required for Q_OBJECT in a .cpp file without a matching .h
#include "SpelbookPlugin.moc"
