package io.github.akarachen.fakes3.junit;

import io.github.akarachen.fakes3.FakeS3;
import org.junit.jupiter.api.extension.ExtensionContext;
import org.junit.jupiter.api.extension.ParameterContext;
import org.junit.jupiter.api.extension.ParameterResolver;

/**
 * Injects a running {@link FakeS3} into test methods, lifecycle methods and nested tests.
 *
 * <p>A {@code FakeS3} parameter on a test method or {@code @BeforeEach} method gets a server of its
 * own for that test, stopped when the test finishes, whether it passed or not. A {@code FakeS3}
 * parameter on a {@code @BeforeAll} method starts one server for the whole class instead, and the
 * test methods of that class receive that same server.
 *
 * <pre>{@code
 * @ExtendWith(FakeS3Extension.class)
 * class UploadTest {
 *   @Test
 *   void uploads(FakeS3 s3) { ... }
 * }
 * }</pre>
 *
 * <p>To configure the servers, register an instance:
 *
 * <pre>{@code
 * @RegisterExtension
 * static FakeS3Extension s3 = new FakeS3Extension(FakeS3.builder().auth(true));
 * }</pre>
 */
public class FakeS3Extension implements ParameterResolver {
  private static final ExtensionContext.Namespace NAMESPACE =
      ExtensionContext.Namespace.create(FakeS3Extension.class);

  private final FakeS3.Builder builder;

  /** Servers with the default settings. */
  public FakeS3Extension() {
    this(FakeS3.builder());
  }

  /** Servers started from {@code builder}. */
  public FakeS3Extension(FakeS3.Builder builder) {
    this.builder = builder;
  }

  @Override
  public boolean supportsParameter(ParameterContext parameter, ExtensionContext context) {
    return parameter.getParameter().getType() == FakeS3.class;
  }

  @Override
  public Object resolveParameter(ParameterContext parameter, ExtensionContext context) {
    // The store looks in the enclosing contexts first, so a server started for the class (from
    // @BeforeAll) is shared by its tests, and otherwise each test starts its own.
    return context
        .getStore(NAMESPACE)
        .getOrComputeIfAbsent(this, k -> new Running(builder.start()), Running.class)
        .server;
  }

  /** Stops the server when its test or class finishes. */
  private static final class Running implements ExtensionContext.Store.CloseableResource {
    final FakeS3 server;

    Running(FakeS3 server) {
      this.server = server;
    }

    @Override
    public void close() {
      server.close();
    }
  }
}
