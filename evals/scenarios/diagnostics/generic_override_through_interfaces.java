class Main {
    interface Component {
        <S> S getSPI(Class<S> type);
    }

    interface Registry extends Component {}

    interface ComponentEx extends Component {
        <E> Iterable<E> getIterableSPI(Class<E> type);
    }

    static abstract class Container implements Registry, ComponentEx {
        public <S> S getSPI(Class<S> type) {
            return null;
        }
    }

    static class Loader {}

    static class UrlLoader extends Loader {
        UrlLoader(Loader parent) {}

        UrlLoader(Container container) {
            this((container != null) ? container.getSPI(Loader.class) : null);
        }
    }

    static class Ambiguous {
        Ambiguous(Loader loader) {}

        Ambiguous(Container container) {}

        static Ambiguous make() {
            return new /*@ambiguous*/Ambiguous(null);
        }
    }

    Loader direct(Container container) {
        var /*@spi*/spi = container.getSPI(Loader.class);
        return spi;
    }
}
//? hover spi ~ "Loader spi"
//? diag ambiguous-call ambiguous
//? diag exact
