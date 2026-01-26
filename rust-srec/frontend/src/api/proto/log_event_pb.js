/*eslint-disable block-scoped-var, id-length, no-control-regex, no-magic-numbers, no-prototype-builtins, no-redeclare, no-shadow, no-var, sort-vars*/
import * as $protobuf from 'protobufjs/minimal';

// Common aliases
const $Reader = $protobuf.Reader,
  $Writer = $protobuf.Writer,
  $util = $protobuf.util;

// Exported root namespace
const $root = $protobuf.roots['default'] || ($protobuf.roots['default'] = {});

export const log_event = ($root.log_event = (() => {
  /**
   * Namespace log_event.
   * @exports log_event
   * @namespace
   */
  const log_event = {};

  /**
   * LogLevel enum.
   * @name log_event.LogLevel
   * @enum {number}
   * @property {number} LOG_LEVEL_UNSPECIFIED=0 LOG_LEVEL_UNSPECIFIED value
   * @property {number} LOG_LEVEL_TRACE=1 LOG_LEVEL_TRACE value
   * @property {number} LOG_LEVEL_DEBUG=2 LOG_LEVEL_DEBUG value
   * @property {number} LOG_LEVEL_INFO=3 LOG_LEVEL_INFO value
   * @property {number} LOG_LEVEL_WARN=4 LOG_LEVEL_WARN value
   * @property {number} LOG_LEVEL_ERROR=5 LOG_LEVEL_ERROR value
   */
  log_event.LogLevel = (function () {
    const valuesById = {},
      values = Object.create(valuesById);
    values[(valuesById[0] = 'LOG_LEVEL_UNSPECIFIED')] = 0;
    values[(valuesById[1] = 'LOG_LEVEL_TRACE')] = 1;
    values[(valuesById[2] = 'LOG_LEVEL_DEBUG')] = 2;
    values[(valuesById[3] = 'LOG_LEVEL_INFO')] = 3;
    values[(valuesById[4] = 'LOG_LEVEL_WARN')] = 4;
    values[(valuesById[5] = 'LOG_LEVEL_ERROR')] = 5;
    return values;
  })();

  /**
   * EventType enum.
   * @name log_event.EventType
   * @enum {number}
   * @property {number} EVENT_TYPE_UNSPECIFIED=0 EVENT_TYPE_UNSPECIFIED value
   * @property {number} EVENT_TYPE_LOG=1 EVENT_TYPE_LOG value
   * @property {number} EVENT_TYPE_ERROR=2 EVENT_TYPE_ERROR value
   */
  log_event.EventType = (function () {
    const valuesById = {},
      values = Object.create(valuesById);
    values[(valuesById[0] = 'EVENT_TYPE_UNSPECIFIED')] = 0;
    values[(valuesById[1] = 'EVENT_TYPE_LOG')] = 1;
    values[(valuesById[2] = 'EVENT_TYPE_ERROR')] = 2;
    return values;
  })();

  log_event.WsMessage = (function () {
    /**
     * Properties of a WsMessage.
     * @memberof log_event
     * @interface IWsMessage
     * @property {log_event.EventType|null} [eventType] WsMessage eventType
     * @property {log_event.ILogEvent|null} [log] WsMessage log
     * @property {log_event.IErrorPayload|null} [error] WsMessage error
     */

    /**
     * Constructs a new WsMessage.
     * @memberof log_event
     * @classdesc Represents a WsMessage.
     * @implements IWsMessage
     * @constructor
     * @param {log_event.IWsMessage=} [properties] Properties to set
     */
    function WsMessage(properties) {
      if (properties)
        for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
          if (properties[keys[i]] != null) this[keys[i]] = properties[keys[i]];
    }

    /**
     * WsMessage eventType.
     * @member {log_event.EventType} eventType
     * @memberof log_event.WsMessage
     * @instance
     */
    WsMessage.prototype.eventType = 0;

    /**
     * WsMessage log.
     * @member {log_event.ILogEvent|null|undefined} log
     * @memberof log_event.WsMessage
     * @instance
     */
    WsMessage.prototype.log = null;

    /**
     * WsMessage error.
     * @member {log_event.IErrorPayload|null|undefined} error
     * @memberof log_event.WsMessage
     * @instance
     */
    WsMessage.prototype.error = null;

    // OneOf field names bound to virtual getters and setters
    let $oneOfFields;

    /**
     * WsMessage payload.
     * @member {"log"|"error"|undefined} payload
     * @memberof log_event.WsMessage
     * @instance
     */
    Object.defineProperty(WsMessage.prototype, 'payload', {
      get: $util.oneOfGetter(($oneOfFields = ['log', 'error'])),
      set: $util.oneOfSetter($oneOfFields),
    });

    /**
     * Creates a new WsMessage instance using the specified properties.
     * @function create
     * @memberof log_event.WsMessage
     * @static
     * @param {log_event.IWsMessage=} [properties] Properties to set
     * @returns {log_event.WsMessage} WsMessage instance
     */
    WsMessage.create = function create(properties) {
      return new WsMessage(properties);
    };

    /**
     * Encodes the specified WsMessage message. Does not implicitly {@link log_event.WsMessage.verify|verify} messages.
     * @function encode
     * @memberof log_event.WsMessage
     * @static
     * @param {log_event.IWsMessage} message WsMessage message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    WsMessage.encode = function encode(message, writer) {
      if (!writer) writer = $Writer.create();
      if (
        message.eventType != null &&
        Object.hasOwnProperty.call(message, 'eventType')
      )
        writer.uint32(/* id 1, wireType 0 =*/ 8).int32(message.eventType);
      if (message.log != null && Object.hasOwnProperty.call(message, 'log'))
        $root.log_event.LogEvent.encode(
          message.log,
          writer.uint32(/* id 2, wireType 2 =*/ 18).fork(),
        ).ldelim();
      if (message.error != null && Object.hasOwnProperty.call(message, 'error'))
        $root.log_event.ErrorPayload.encode(
          message.error,
          writer.uint32(/* id 3, wireType 2 =*/ 26).fork(),
        ).ldelim();
      return writer;
    };

    /**
     * Encodes the specified WsMessage message, length delimited. Does not implicitly {@link log_event.WsMessage.verify|verify} messages.
     * @function encodeDelimited
     * @memberof log_event.WsMessage
     * @static
     * @param {log_event.IWsMessage} message WsMessage message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    WsMessage.encodeDelimited = function encodeDelimited(message, writer) {
      return this.encode(message, writer).ldelim();
    };

    /**
     * Decodes a WsMessage message from the specified reader or buffer.
     * @function decode
     * @memberof log_event.WsMessage
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @param {number} [length] Message length if known beforehand
     * @returns {log_event.WsMessage} WsMessage
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    WsMessage.decode = function decode(reader, length, error) {
      if (!(reader instanceof $Reader)) reader = $Reader.create(reader);
      let end = length === undefined ? reader.len : reader.pos + length,
        message = new $root.log_event.WsMessage();
      while (reader.pos < end) {
        let tag = reader.uint32();
        if (tag === error) break;
        switch (tag >>> 3) {
          case 1: {
            message.eventType = reader.int32();
            break;
          }
          case 2: {
            message.log = $root.log_event.LogEvent.decode(
              reader,
              reader.uint32(),
            );
            break;
          }
          case 3: {
            message.error = $root.log_event.ErrorPayload.decode(
              reader,
              reader.uint32(),
            );
            break;
          }
          default:
            reader.skipType(tag & 7);
            break;
        }
      }
      return message;
    };

    /**
     * Decodes a WsMessage message from the specified reader or buffer, length delimited.
     * @function decodeDelimited
     * @memberof log_event.WsMessage
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @returns {log_event.WsMessage} WsMessage
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    WsMessage.decodeDelimited = function decodeDelimited(reader) {
      if (!(reader instanceof $Reader)) reader = new $Reader(reader);
      return this.decode(reader, reader.uint32());
    };

    /**
     * Verifies a WsMessage message.
     * @function verify
     * @memberof log_event.WsMessage
     * @static
     * @param {Object.<string,*>} message Plain object to verify
     * @returns {string|null} `null` if valid, otherwise the reason why it is not
     */
    WsMessage.verify = function verify(message) {
      if (typeof message !== 'object' || message === null)
        return 'object expected';
      let properties = {};
      if (message.eventType != null && message.hasOwnProperty('eventType'))
        switch (message.eventType) {
          default:
            return 'eventType: enum value expected';
          case 0:
          case 1:
          case 2:
            break;
        }
      if (message.log != null && message.hasOwnProperty('log')) {
        properties.payload = 1;
        {
          let error = $root.log_event.LogEvent.verify(message.log);
          if (error) return 'log.' + error;
        }
      }
      if (message.error != null && message.hasOwnProperty('error')) {
        if (properties.payload === 1) return 'payload: multiple values';
        properties.payload = 1;
        {
          let error = $root.log_event.ErrorPayload.verify(message.error);
          if (error) return 'error.' + error;
        }
      }
      return null;
    };

    /**
     * Creates a WsMessage message from a plain object. Also converts values to their respective internal types.
     * @function fromObject
     * @memberof log_event.WsMessage
     * @static
     * @param {Object.<string,*>} object Plain object
     * @returns {log_event.WsMessage} WsMessage
     */
    WsMessage.fromObject = function fromObject(object) {
      if (object instanceof $root.log_event.WsMessage) return object;
      let message = new $root.log_event.WsMessage();
      switch (object.eventType) {
        default:
          if (typeof object.eventType === 'number') {
            message.eventType = object.eventType;
            break;
          }
          break;
        case 'EVENT_TYPE_UNSPECIFIED':
        case 0:
          message.eventType = 0;
          break;
        case 'EVENT_TYPE_LOG':
        case 1:
          message.eventType = 1;
          break;
        case 'EVENT_TYPE_ERROR':
        case 2:
          message.eventType = 2;
          break;
      }
      if (object.log != null) {
        if (typeof object.log !== 'object')
          throw TypeError('.log_event.WsMessage.log: object expected');
        message.log = $root.log_event.LogEvent.fromObject(object.log);
      }
      if (object.error != null) {
        if (typeof object.error !== 'object')
          throw TypeError('.log_event.WsMessage.error: object expected');
        message.error = $root.log_event.ErrorPayload.fromObject(object.error);
      }
      return message;
    };

    /**
     * Creates a plain object from a WsMessage message. Also converts values to other types if specified.
     * @function toObject
     * @memberof log_event.WsMessage
     * @static
     * @param {log_event.WsMessage} message WsMessage
     * @param {$protobuf.IConversionOptions} [options] Conversion options
     * @returns {Object.<string,*>} Plain object
     */
    WsMessage.toObject = function toObject(message, options) {
      if (!options) options = {};
      let object = {};
      if (options.defaults)
        object.eventType =
          options.enums === String ? 'EVENT_TYPE_UNSPECIFIED' : 0;
      if (message.eventType != null && message.hasOwnProperty('eventType'))
        object.eventType =
          options.enums === String
            ? $root.log_event.EventType[message.eventType] === undefined
              ? message.eventType
              : $root.log_event.EventType[message.eventType]
            : message.eventType;
      if (message.log != null && message.hasOwnProperty('log')) {
        object.log = $root.log_event.LogEvent.toObject(message.log, options);
        if (options.oneofs) object.payload = 'log';
      }
      if (message.error != null && message.hasOwnProperty('error')) {
        object.error = $root.log_event.ErrorPayload.toObject(
          message.error,
          options,
        );
        if (options.oneofs) object.payload = 'error';
      }
      return object;
    };

    /**
     * Converts this WsMessage to JSON.
     * @function toJSON
     * @memberof log_event.WsMessage
     * @instance
     * @returns {Object.<string,*>} JSON object
     */
    WsMessage.prototype.toJSON = function toJSON() {
      return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
    };

    /**
     * Gets the default type url for WsMessage
     * @function getTypeUrl
     * @memberof log_event.WsMessage
     * @static
     * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
     * @returns {string} The default type url
     */
    WsMessage.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
      if (typeUrlPrefix === undefined) {
        typeUrlPrefix = 'type.googleapis.com';
      }
      return typeUrlPrefix + '/log_event.WsMessage';
    };

    return WsMessage;
  })();

  log_event.LogEvent = (function () {
    /**
     * Properties of a LogEvent.
     * @memberof log_event
     * @interface ILogEvent
     * @property {number|Long|null} [timestampMs] LogEvent timestampMs
     * @property {log_event.LogLevel|null} [level] LogEvent level
     * @property {string|null} [target] LogEvent target
     * @property {string|null} [message] LogEvent message
     */

    /**
     * Constructs a new LogEvent.
     * @memberof log_event
     * @classdesc Represents a LogEvent.
     * @implements ILogEvent
     * @constructor
     * @param {log_event.ILogEvent=} [properties] Properties to set
     */
    function LogEvent(properties) {
      if (properties)
        for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
          if (properties[keys[i]] != null) this[keys[i]] = properties[keys[i]];
    }

    /**
     * LogEvent timestampMs.
     * @member {number|Long} timestampMs
     * @memberof log_event.LogEvent
     * @instance
     */
    LogEvent.prototype.timestampMs = $util.Long
      ? $util.Long.fromBits(0, 0, false)
      : 0;

    /**
     * LogEvent level.
     * @member {log_event.LogLevel} level
     * @memberof log_event.LogEvent
     * @instance
     */
    LogEvent.prototype.level = 0;

    /**
     * LogEvent target.
     * @member {string} target
     * @memberof log_event.LogEvent
     * @instance
     */
    LogEvent.prototype.target = '';

    /**
     * LogEvent message.
     * @member {string} message
     * @memberof log_event.LogEvent
     * @instance
     */
    LogEvent.prototype.message = '';

    /**
     * Creates a new LogEvent instance using the specified properties.
     * @function create
     * @memberof log_event.LogEvent
     * @static
     * @param {log_event.ILogEvent=} [properties] Properties to set
     * @returns {log_event.LogEvent} LogEvent instance
     */
    LogEvent.create = function create(properties) {
      return new LogEvent(properties);
    };

    /**
     * Encodes the specified LogEvent message. Does not implicitly {@link log_event.LogEvent.verify|verify} messages.
     * @function encode
     * @memberof log_event.LogEvent
     * @static
     * @param {log_event.ILogEvent} message LogEvent message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    LogEvent.encode = function encode(message, writer) {
      if (!writer) writer = $Writer.create();
      if (
        message.timestampMs != null &&
        Object.hasOwnProperty.call(message, 'timestampMs')
      )
        writer.uint32(/* id 1, wireType 0 =*/ 8).int64(message.timestampMs);
      if (message.level != null && Object.hasOwnProperty.call(message, 'level'))
        writer.uint32(/* id 2, wireType 0 =*/ 16).int32(message.level);
      if (
        message.target != null &&
        Object.hasOwnProperty.call(message, 'target')
      )
        writer.uint32(/* id 3, wireType 2 =*/ 26).string(message.target);
      if (
        message.message != null &&
        Object.hasOwnProperty.call(message, 'message')
      )
        writer.uint32(/* id 4, wireType 2 =*/ 34).string(message.message);
      return writer;
    };

    /**
     * Encodes the specified LogEvent message, length delimited. Does not implicitly {@link log_event.LogEvent.verify|verify} messages.
     * @function encodeDelimited
     * @memberof log_event.LogEvent
     * @static
     * @param {log_event.ILogEvent} message LogEvent message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    LogEvent.encodeDelimited = function encodeDelimited(message, writer) {
      return this.encode(message, writer).ldelim();
    };

    /**
     * Decodes a LogEvent message from the specified reader or buffer.
     * @function decode
     * @memberof log_event.LogEvent
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @param {number} [length] Message length if known beforehand
     * @returns {log_event.LogEvent} LogEvent
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    LogEvent.decode = function decode(reader, length, error) {
      if (!(reader instanceof $Reader)) reader = $Reader.create(reader);
      let end = length === undefined ? reader.len : reader.pos + length,
        message = new $root.log_event.LogEvent();
      while (reader.pos < end) {
        let tag = reader.uint32();
        if (tag === error) break;
        switch (tag >>> 3) {
          case 1: {
            message.timestampMs = reader.int64();
            break;
          }
          case 2: {
            message.level = reader.int32();
            break;
          }
          case 3: {
            message.target = reader.string();
            break;
          }
          case 4: {
            message.message = reader.string();
            break;
          }
          default:
            reader.skipType(tag & 7);
            break;
        }
      }
      return message;
    };

    /**
     * Decodes a LogEvent message from the specified reader or buffer, length delimited.
     * @function decodeDelimited
     * @memberof log_event.LogEvent
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @returns {log_event.LogEvent} LogEvent
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    LogEvent.decodeDelimited = function decodeDelimited(reader) {
      if (!(reader instanceof $Reader)) reader = new $Reader(reader);
      return this.decode(reader, reader.uint32());
    };

    /**
     * Verifies a LogEvent message.
     * @function verify
     * @memberof log_event.LogEvent
     * @static
     * @param {Object.<string,*>} message Plain object to verify
     * @returns {string|null} `null` if valid, otherwise the reason why it is not
     */
    LogEvent.verify = function verify(message) {
      if (typeof message !== 'object' || message === null)
        return 'object expected';
      if (message.timestampMs != null && message.hasOwnProperty('timestampMs'))
        if (
          !$util.isInteger(message.timestampMs) &&
          !(
            message.timestampMs &&
            $util.isInteger(message.timestampMs.low) &&
            $util.isInteger(message.timestampMs.high)
          )
        )
          return 'timestampMs: integer|Long expected';
      if (message.level != null && message.hasOwnProperty('level'))
        switch (message.level) {
          default:
            return 'level: enum value expected';
          case 0:
          case 1:
          case 2:
          case 3:
          case 4:
          case 5:
            break;
        }
      if (message.target != null && message.hasOwnProperty('target'))
        if (!$util.isString(message.target)) return 'target: string expected';
      if (message.message != null && message.hasOwnProperty('message'))
        if (!$util.isString(message.message)) return 'message: string expected';
      return null;
    };

    /**
     * Creates a LogEvent message from a plain object. Also converts values to their respective internal types.
     * @function fromObject
     * @memberof log_event.LogEvent
     * @static
     * @param {Object.<string,*>} object Plain object
     * @returns {log_event.LogEvent} LogEvent
     */
    LogEvent.fromObject = function fromObject(object) {
      if (object instanceof $root.log_event.LogEvent) return object;
      let message = new $root.log_event.LogEvent();
      if (object.timestampMs != null)
        if ($util.Long)
          (message.timestampMs = $util.Long.fromValue(
            object.timestampMs,
          )).unsigned = false;
        else if (typeof object.timestampMs === 'string')
          message.timestampMs = parseInt(object.timestampMs, 10);
        else if (typeof object.timestampMs === 'number')
          message.timestampMs = object.timestampMs;
        else if (typeof object.timestampMs === 'object')
          message.timestampMs = new $util.LongBits(
            object.timestampMs.low >>> 0,
            object.timestampMs.high >>> 0,
          ).toNumber();
      switch (object.level) {
        default:
          if (typeof object.level === 'number') {
            message.level = object.level;
            break;
          }
          break;
        case 'LOG_LEVEL_UNSPECIFIED':
        case 0:
          message.level = 0;
          break;
        case 'LOG_LEVEL_TRACE':
        case 1:
          message.level = 1;
          break;
        case 'LOG_LEVEL_DEBUG':
        case 2:
          message.level = 2;
          break;
        case 'LOG_LEVEL_INFO':
        case 3:
          message.level = 3;
          break;
        case 'LOG_LEVEL_WARN':
        case 4:
          message.level = 4;
          break;
        case 'LOG_LEVEL_ERROR':
        case 5:
          message.level = 5;
          break;
      }
      if (object.target != null) message.target = String(object.target);
      if (object.message != null) message.message = String(object.message);
      return message;
    };

    /**
     * Creates a plain object from a LogEvent message. Also converts values to other types if specified.
     * @function toObject
     * @memberof log_event.LogEvent
     * @static
     * @param {log_event.LogEvent} message LogEvent
     * @param {$protobuf.IConversionOptions} [options] Conversion options
     * @returns {Object.<string,*>} Plain object
     */
    LogEvent.toObject = function toObject(message, options) {
      if (!options) options = {};
      let object = {};
      if (options.defaults) {
        if ($util.Long) {
          let long = new $util.Long(0, 0, false);
          object.timestampMs =
            options.longs === String
              ? long.toString()
              : options.longs === Number
                ? long.toNumber()
                : long;
        } else object.timestampMs = options.longs === String ? '0' : 0;
        object.level = options.enums === String ? 'LOG_LEVEL_UNSPECIFIED' : 0;
        object.target = '';
        object.message = '';
      }
      if (message.timestampMs != null && message.hasOwnProperty('timestampMs'))
        if (typeof message.timestampMs === 'number')
          object.timestampMs =
            options.longs === String
              ? String(message.timestampMs)
              : message.timestampMs;
        else
          object.timestampMs =
            options.longs === String
              ? $util.Long.prototype.toString.call(message.timestampMs)
              : options.longs === Number
                ? new $util.LongBits(
                    message.timestampMs.low >>> 0,
                    message.timestampMs.high >>> 0,
                  ).toNumber()
                : message.timestampMs;
      if (message.level != null && message.hasOwnProperty('level'))
        object.level =
          options.enums === String
            ? $root.log_event.LogLevel[message.level] === undefined
              ? message.level
              : $root.log_event.LogLevel[message.level]
            : message.level;
      if (message.target != null && message.hasOwnProperty('target'))
        object.target = message.target;
      if (message.message != null && message.hasOwnProperty('message'))
        object.message = message.message;
      return object;
    };

    /**
     * Converts this LogEvent to JSON.
     * @function toJSON
     * @memberof log_event.LogEvent
     * @instance
     * @returns {Object.<string,*>} JSON object
     */
    LogEvent.prototype.toJSON = function toJSON() {
      return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
    };

    /**
     * Gets the default type url for LogEvent
     * @function getTypeUrl
     * @memberof log_event.LogEvent
     * @static
     * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
     * @returns {string} The default type url
     */
    LogEvent.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
      if (typeUrlPrefix === undefined) {
        typeUrlPrefix = 'type.googleapis.com';
      }
      return typeUrlPrefix + '/log_event.LogEvent';
    };

    return LogEvent;
  })();

  log_event.ErrorPayload = (function () {
    /**
     * Properties of an ErrorPayload.
     * @memberof log_event
     * @interface IErrorPayload
     * @property {string|null} [code] ErrorPayload code
     * @property {string|null} [message] ErrorPayload message
     */

    /**
     * Constructs a new ErrorPayload.
     * @memberof log_event
     * @classdesc Represents an ErrorPayload.
     * @implements IErrorPayload
     * @constructor
     * @param {log_event.IErrorPayload=} [properties] Properties to set
     */
    function ErrorPayload(properties) {
      if (properties)
        for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
          if (properties[keys[i]] != null) this[keys[i]] = properties[keys[i]];
    }

    /**
     * ErrorPayload code.
     * @member {string} code
     * @memberof log_event.ErrorPayload
     * @instance
     */
    ErrorPayload.prototype.code = '';

    /**
     * ErrorPayload message.
     * @member {string} message
     * @memberof log_event.ErrorPayload
     * @instance
     */
    ErrorPayload.prototype.message = '';

    /**
     * Creates a new ErrorPayload instance using the specified properties.
     * @function create
     * @memberof log_event.ErrorPayload
     * @static
     * @param {log_event.IErrorPayload=} [properties] Properties to set
     * @returns {log_event.ErrorPayload} ErrorPayload instance
     */
    ErrorPayload.create = function create(properties) {
      return new ErrorPayload(properties);
    };

    /**
     * Encodes the specified ErrorPayload message. Does not implicitly {@link log_event.ErrorPayload.verify|verify} messages.
     * @function encode
     * @memberof log_event.ErrorPayload
     * @static
     * @param {log_event.IErrorPayload} message ErrorPayload message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    ErrorPayload.encode = function encode(message, writer) {
      if (!writer) writer = $Writer.create();
      if (message.code != null && Object.hasOwnProperty.call(message, 'code'))
        writer.uint32(/* id 1, wireType 2 =*/ 10).string(message.code);
      if (
        message.message != null &&
        Object.hasOwnProperty.call(message, 'message')
      )
        writer.uint32(/* id 2, wireType 2 =*/ 18).string(message.message);
      return writer;
    };

    /**
     * Encodes the specified ErrorPayload message, length delimited. Does not implicitly {@link log_event.ErrorPayload.verify|verify} messages.
     * @function encodeDelimited
     * @memberof log_event.ErrorPayload
     * @static
     * @param {log_event.IErrorPayload} message ErrorPayload message or plain object to encode
     * @param {$protobuf.Writer} [writer] Writer to encode to
     * @returns {$protobuf.Writer} Writer
     */
    ErrorPayload.encodeDelimited = function encodeDelimited(message, writer) {
      return this.encode(message, writer).ldelim();
    };

    /**
     * Decodes an ErrorPayload message from the specified reader or buffer.
     * @function decode
     * @memberof log_event.ErrorPayload
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @param {number} [length] Message length if known beforehand
     * @returns {log_event.ErrorPayload} ErrorPayload
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    ErrorPayload.decode = function decode(reader, length, error) {
      if (!(reader instanceof $Reader)) reader = $Reader.create(reader);
      let end = length === undefined ? reader.len : reader.pos + length,
        message = new $root.log_event.ErrorPayload();
      while (reader.pos < end) {
        let tag = reader.uint32();
        if (tag === error) break;
        switch (tag >>> 3) {
          case 1: {
            message.code = reader.string();
            break;
          }
          case 2: {
            message.message = reader.string();
            break;
          }
          default:
            reader.skipType(tag & 7);
            break;
        }
      }
      return message;
    };

    /**
     * Decodes an ErrorPayload message from the specified reader or buffer, length delimited.
     * @function decodeDelimited
     * @memberof log_event.ErrorPayload
     * @static
     * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
     * @returns {log_event.ErrorPayload} ErrorPayload
     * @throws {Error} If the payload is not a reader or valid buffer
     * @throws {$protobuf.util.ProtocolError} If required fields are missing
     */
    ErrorPayload.decodeDelimited = function decodeDelimited(reader) {
      if (!(reader instanceof $Reader)) reader = new $Reader(reader);
      return this.decode(reader, reader.uint32());
    };

    /**
     * Verifies an ErrorPayload message.
     * @function verify
     * @memberof log_event.ErrorPayload
     * @static
     * @param {Object.<string,*>} message Plain object to verify
     * @returns {string|null} `null` if valid, otherwise the reason why it is not
     */
    ErrorPayload.verify = function verify(message) {
      if (typeof message !== 'object' || message === null)
        return 'object expected';
      if (message.code != null && message.hasOwnProperty('code'))
        if (!$util.isString(message.code)) return 'code: string expected';
      if (message.message != null && message.hasOwnProperty('message'))
        if (!$util.isString(message.message)) return 'message: string expected';
      return null;
    };

    /**
     * Creates an ErrorPayload message from a plain object. Also converts values to their respective internal types.
     * @function fromObject
     * @memberof log_event.ErrorPayload
     * @static
     * @param {Object.<string,*>} object Plain object
     * @returns {log_event.ErrorPayload} ErrorPayload
     */
    ErrorPayload.fromObject = function fromObject(object) {
      if (object instanceof $root.log_event.ErrorPayload) return object;
      let message = new $root.log_event.ErrorPayload();
      if (object.code != null) message.code = String(object.code);
      if (object.message != null) message.message = String(object.message);
      return message;
    };

    /**
     * Creates a plain object from an ErrorPayload message. Also converts values to other types if specified.
     * @function toObject
     * @memberof log_event.ErrorPayload
     * @static
     * @param {log_event.ErrorPayload} message ErrorPayload
     * @param {$protobuf.IConversionOptions} [options] Conversion options
     * @returns {Object.<string,*>} Plain object
     */
    ErrorPayload.toObject = function toObject(message, options) {
      if (!options) options = {};
      let object = {};
      if (options.defaults) {
        object.code = '';
        object.message = '';
      }
      if (message.code != null && message.hasOwnProperty('code'))
        object.code = message.code;
      if (message.message != null && message.hasOwnProperty('message'))
        object.message = message.message;
      return object;
    };

    /**
     * Converts this ErrorPayload to JSON.
     * @function toJSON
     * @memberof log_event.ErrorPayload
     * @instance
     * @returns {Object.<string,*>} JSON object
     */
    ErrorPayload.prototype.toJSON = function toJSON() {
      return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
    };

    /**
     * Gets the default type url for ErrorPayload
     * @function getTypeUrl
     * @memberof log_event.ErrorPayload
     * @static
     * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
     * @returns {string} The default type url
     */
    ErrorPayload.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
      if (typeUrlPrefix === undefined) {
        typeUrlPrefix = 'type.googleapis.com';
      }
      return typeUrlPrefix + '/log_event.ErrorPayload';
    };

    return ErrorPayload;
  })();

  return log_event;
})());

export { $root as default };
